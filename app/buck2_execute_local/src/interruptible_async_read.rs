/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io;
use std::io::Cursor;
use std::io::Read;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::future::Future;
use futures::future::FutureExt;
use pin_project::pin_project;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::ReadBuf;
use tokio_util::codec::FramedRead;

/// This trait represents the ability to transition a Reader (R) to a Drainer (Self). Both are
/// [AsyncRead], but we expect the Drainer (which implements this trait) to not wait longer for
/// more data to be produced.
pub(crate) trait DrainerFromReader<R> {
    fn from_reader(reader: R) -> Self;
}

/// This trait represents a AsyncRead that can be told to interrupt (and transition to draining).
pub(crate) trait InterruptNotifiable {
    fn notify_interrupt(self: Pin<&mut Self>);
}

/// An [AsyncRead] that can be interrupted. The underlying [AsyncRead] will be polled while the
/// [Future] provided in interrupt hasn't completed. When said future completes, then we will
/// proceed to "drain" the reader, which means reading the data that is there but not waiting for
/// any further data to get written.
#[pin_project]
pub(crate) struct InterruptibleAsyncRead<R, D> {
    state: InterruptibleAsyncReadState<R, D>,
}

enum InterruptibleAsyncReadState<R, D> {
    Reading(R),
    Draining(D),
}

impl<R, D> InterruptibleAsyncRead<R, D>
where
    D: DrainerFromReader<R>,
{
    pub fn new(reader: R) -> Self {
        Self {
            state: InterruptibleAsyncReadState::Reading(reader),
        }
    }
}

impl<R, D> AsyncRead for InterruptibleAsyncRead<R, D>
where
    R: AsyncRead + Unpin,
    D: AsyncRead + Unpin + DrainerFromReader<R>,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.project();

        match this.state {
            InterruptibleAsyncReadState::Reading(reader) => Pin::new(reader).poll_read(cx, buf),
            InterruptibleAsyncReadState::Draining(drainer) => Pin::new(drainer).poll_read(cx, buf),
        }
    }
}

#[cfg(unix)]
mod unix_non_blocking_drainer {
    use std::os::fd::AsFd;

    use nix::unistd;

    use super::*;

    /// Perform sync reads on an existing reader. We use this to bypass Tokio when draining an
    /// InterruptibleAsyncRead after it's been interrupted. This lets us ensure that even if Tokio
    /// hasn't completed `select()` on the pipe we want to drain, we'll still get to execute
    /// `read()` (and potentially get WouldBlock if there is nothing to read and the pipe isn't
    /// ready).
    pub(crate) struct UnixNonBlockingDrainer<R> {
        inner: R,
    }

    impl<R> DrainerFromReader<R> for UnixNonBlockingDrainer<R>
    where
        R: AsFd + Send + 'static,
    {
        fn from_reader(reader: R) -> Self {
            UnixNonBlockingDrainer { inner: reader }
        }
    }

    impl<R: AsFd> AsyncRead for UnixNonBlockingDrainer<R> {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(
                match unistd::read(&self.inner, buf.initialize_unfilled()).map_err(io::Error::from)
                {
                    Err(e) => {
                        if e.kind() == io::ErrorKind::WouldBlock {
                            tracing::debug!("Child did not close its pipe");
                            Ok(())
                        } else {
                            Err(e)
                        }
                    }
                    Ok(n) => {
                        buf.advance(n);
                        Ok(())
                    }
                },
            )
        }
    }
}

#[cfg(unix)]
pub(crate) use unix_non_blocking_drainer::*;

#[pin_project]
pub(crate) struct TimeoutDrainer<R> {
    state: TimeoutDrainerState,
    // To have a generic parameter like UnixNonBlockingDrainer does.
    _phantom: PhantomData<R>,
}

enum TimeoutDrainerState {
    Waiting(BoxFuture<'static, io::Result<Vec<u8>>>),
    Draining(Cursor<Vec<u8>>),
}

impl<R> DrainerFromReader<R> for TimeoutDrainer<R>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    fn from_reader(mut reader: R) -> Self {
        let fut = async move {
            let mut buff = Vec::new();
            {
                let do_read = reader.read_to_end(&mut buff);
                futures::pin_mut!(do_read);
                match tokio::time::timeout(Duration::from_secs(1), do_read).await {
                    Ok(Err(e)) => return Err(e),
                    Err(..) => {
                        tracing::debug!("Child did not close its pipe");
                    }
                    Ok(Ok(..)) => {}
                }
            }
            Ok(buff)
        };

        Self {
            state: TimeoutDrainerState::Waiting(fut.boxed()),
            _phantom: PhantomData,
        }
    }
}

impl<R> AsyncRead for TimeoutDrainer<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.project();

        loop {
            match &mut *this.state {
                TimeoutDrainerState::Waiting(fut) => {
                    let bytes = futures::ready!(Pin::new(fut).poll(cx))?;
                    *this.state = TimeoutDrainerState::Draining(Cursor::new(bytes));
                }
                TimeoutDrainerState::Draining(bytes) => {
                    let n = Read::read(bytes, buf.initialize_unfilled())?;
                    buf.advance(n);
                    break Poll::Ready(Ok(()));
                }
            }
        }
    }
}

impl<R, D> InterruptNotifiable for InterruptibleAsyncRead<R, D>
where
    D: DrainerFromReader<R>,
{
    fn notify_interrupt(self: Pin<&mut Self>) {
        let this = self.project();

        take_mut::take(this.state, |state| match state {
            InterruptibleAsyncReadState::Reading(reader) => {
                InterruptibleAsyncReadState::Draining(D::from_reader(reader))
            }
            _ => unreachable!(),
        });
    }
}

impl<S1, S2> InterruptNotifiable for futures::stream::Select<S1, S2>
where
    S1: InterruptNotifiable,
    S2: InterruptNotifiable,
{
    fn notify_interrupt(self: Pin<&mut Self>) {
        let (s1, s2) = self.get_pin_mut();
        s1.notify_interrupt();
        s2.notify_interrupt();
    }
}

impl<S, F> InterruptNotifiable for futures::stream::Map<S, F>
where
    S: InterruptNotifiable,
{
    fn notify_interrupt(self: Pin<&mut Self>) {
        self.get_pin_mut().notify_interrupt();
    }
}

impl<T, D> InterruptNotifiable for FramedRead<T, D>
where
    T: InterruptNotifiable,
{
    fn notify_interrupt(self: Pin<&mut Self>) {
        self.get_pin_mut().notify_interrupt();
    }
}

impl<T> InterruptNotifiable for futures::stream::Empty<T> {
    fn notify_interrupt(self: Pin<&mut Self>) {}
}

impl<L, R> InterruptNotifiable for futures::future::Either<L, R>
where
    L: InterruptNotifiable,
    R: InterruptNotifiable,
{
    fn notify_interrupt(self: Pin<&mut Self>) {
        match self.as_pin_mut() {
            futures::future::Either::Left(s) => s.notify_interrupt(),
            futures::future::Either::Right(s) => s.notify_interrupt(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod interruptible_async_read {
        use std::sync::Arc;
        use std::sync::Mutex;

        use assert_matches::assert_matches;
        use dupe::Dupe;
        use futures::stream::Stream;
        use tokio_util::codec::BytesCodec;

        use super::*;

        #[derive(Copy, Clone, Dupe)]
        enum StubAsyncReadState {
            Done,
            Pending,
            Ready(u8),
        }

        /// A stub AsyncRead implementation
        #[derive(Clone, Dupe)]
        #[pin_project]
        struct StubAsyncRead {
            state: Arc<Mutex<StubAsyncReadState>>,
            drainer: bool,
        }

        impl DrainerFromReader<StubAsyncRead> for StubAsyncRead {
            fn from_reader(reader: StubAsyncRead) -> Self {
                Self {
                    state: reader.state,
                    drainer: true,
                }
            }
        }

        impl StubAsyncRead {
            fn new() -> Self {
                Self {
                    state: Arc::new(Mutex::new(StubAsyncReadState::Pending)),
                    drainer: false,
                }
            }

            /// Push a byte to this. Doesn't check that this makes sense.
            fn push(&self, byte: u8) {
                let mut state = self.state.lock().unwrap();
                *state = StubAsyncReadState::Ready(byte);
            }

            /// Set this to done. Also doesn't check that this makes sense.
            fn done(&self) {
                let mut state = self.state.lock().unwrap();
                *state = StubAsyncReadState::Done;
            }
        }

        impl AsyncRead for StubAsyncRead {
            /// A poll_read implementation for our stub. Note that this never stores any wakers,
            /// that's OK because our tests do explicit poll!().
            fn poll_read(
                self: Pin<&mut Self>,
                _: &mut Context<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                let this = self.project();
                let mut state = this.state.lock().unwrap();

                match *state {
                    StubAsyncReadState::Done => Poll::Ready(Ok(())),
                    StubAsyncReadState::Pending => {
                        if *this.drainer {
                            Poll::Ready(Ok(()))
                        } else {
                            Poll::Pending
                        }
                    }
                    StubAsyncReadState::Ready(b) => {
                        buf.put_slice(&[b]);
                        *state = StubAsyncReadState::Pending;
                        Poll::Ready(Ok(()))
                    }
                }
            }
        }

        #[pin_project]
        struct ReadAll<R, D> {
            #[pin]
            inner: FramedRead<InterruptibleAsyncRead<R, D>, BytesCodec>,

            buff: Vec<u8>,
        }

        impl<R, D> InterruptNotifiable for ReadAll<R, D>
        where
            D: DrainerFromReader<R>,
        {
            fn notify_interrupt(self: Pin<&mut Self>) {
                let this = self.project();
                this.inner.get_pin_mut().notify_interrupt()
            }
        }

        impl<R, D> Future for ReadAll<R, D>
        where
            R: AsyncRead + Unpin,
            D: AsyncRead + Unpin + DrainerFromReader<R>,
        {
            type Output = buck2_error::Result<Vec<u8>>;

            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                let mut this = self.project();

                loop {
                    match futures::ready!(this.inner.as_mut().poll_next(cx)?) {
                        Some(bytes) => this.buff.extend(bytes.as_ref()),
                        None => return Poll::Ready(Ok(std::mem::take(this.buff))),
                    }
                }
            }
        }

        fn read_all<R, D>(read: InterruptibleAsyncRead<R, D>) -> ReadAll<R, D>
        where
            R: AsyncRead + Unpin,
            D: AsyncRead + Unpin + DrainerFromReader<R>,
        {
            ReadAll {
                inner: FramedRead::new(read, BytesCodec::new()),
                buff: Vec::new(),
            }
        }

        #[tokio::test]
        async fn test_drain_eof() {
            let read = StubAsyncRead::new();
            let interruptible = InterruptibleAsyncRead::<_, StubAsyncRead>::new(read.dupe());

            let drain = read_all(interruptible);
            futures::pin_mut!(drain);

            // No bytes, it's pending.
            assert_matches!(futures::poll!(drain.as_mut()), Poll::Pending);

            // Add a byte, still pending, because more may come.
            read.push(b'f');
            assert_matches!(futures::poll!(drain.as_mut()), Poll::Pending);

            // Mark our reader done, we expect this to be ready.
            read.done();
            assert_matches!(futures::poll!(drain.as_mut()), Poll::Ready(Ok(ret)) => {
                assert_eq!(&ret, b"f");
            });
        }

        #[tokio::test]
        async fn test_drain_interrupt() {
            let read = StubAsyncRead::new();
            let interruptible = InterruptibleAsyncRead::<_, StubAsyncRead>::new(read.dupe());

            let drain = read_all(interruptible);
            futures::pin_mut!(drain);

            // No bytes, it's pending.
            assert_matches!(futures::poll!(drain.as_mut()), Poll::Pending);

            // Add a byte, still pending, because more may come.
            read.push(b'f');
            assert_matches!(futures::poll!(drain.as_mut()), Poll::Pending);

            // Close the reader.
            drain.as_mut().notify_interrupt();

            // Mark our reader done, we expect this to be ready.
            assert_matches!(futures::poll!(drain.as_mut()), Poll::Ready(Ok(ret)) => {
                assert_eq!(&ret, b"f");
            });
        }

        #[tokio::test]
        async fn test_drain_finish() {
            let read = StubAsyncRead::new();
            let interruptible = InterruptibleAsyncRead::<_, StubAsyncRead>::new(read.dupe());

            let drain = read_all(interruptible);
            futures::pin_mut!(drain);

            // No bytes, it's pending.
            assert_matches!(futures::poll!(drain.as_mut()), Poll::Pending);

            // Add a byte, still pending, because more may come.
            read.push(b'f');
            assert_matches!(futures::poll!(drain.as_mut()), Poll::Pending);

            // Close the reader.
            drain.as_mut().notify_interrupt();

            // But! Add more stuff.
            read.push(b'o');

            // We now expect this to read what's left.
            assert_matches!(futures::poll!(drain.as_mut()), Poll::Ready(Ok(ret)) => {
                assert_eq!(&ret, b"fo");
            });
        }
    }

    mod timeout_drainer {
        use bytes::Bytes;
        use futures::StreamExt;

        use super::*;

        #[tokio::test]
        async fn test_timeout_drainer() -> buck2_error::Result<()> {
            // 64 bytes of a. Tokio allocates a 32 byte buffer for read_to_end so this is good to
            //    ensure we get 2 reads.
            let s = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

            let reader = tokio_util::io::StreamReader::new(
                tokio_stream::iter(vec![io::Result::Ok(Bytes::from_static(s.as_bytes()))])
                    .chain(futures::stream::pending()),
            );

            let mut drainer = TimeoutDrainer::from_reader(reader);
            let mut buff = Vec::new();
            tokio::time::timeout(Duration::from_secs(5), drainer.read_to_end(&mut buff)).await??;
            assert_eq!(s.as_bytes(), buff.as_slice());

            Ok(())
        }
    }

    #[cfg(unix)]
    mod unix_non_blocking_drainer {
        use std::process::Stdio;

        use buck2_util::process::async_background_command;

        use super::*;

        #[tokio::test]
        async fn test_unix_non_blocking_drainer() -> buck2_error::Result<()> {
            // See above.
            let s = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

            let mut command = async_background_command("sh");
            command
                .args(["-c", &format!("((sleep 10 && echo extra) &) && echo {s}")])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            let mut command = command.spawn()?;

            let stdout = command.stdout.take().unwrap();
            let mut drainer =
                unix_non_blocking_drainer::UnixNonBlockingDrainer::from_reader(stdout);

            command.wait().await?;

            let mut buff = String::new();
            drainer.read_to_string(&mut buff).await?;
            assert_eq!(format!("{s}\n"), buff.as_str());

            Ok(())
        }
    }
}
