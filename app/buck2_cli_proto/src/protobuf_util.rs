/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io::Cursor;

use buck2_error::buck2_error;
use bytes::BytesMut;
use tokio_util::codec::Decoder;

/// Splits length-prefixed protobuf.
pub struct ProtobufSplitter;

impl Decoder for ProtobufSplitter {
    type Item = BytesMut;
    type Error = buck2_error::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let orig_len = src.len();

        let data_length = match prost::decode_length_delimiter(Cursor::new(src.as_mut())) {
            Ok(length) => length,
            Err(..) => {
                // 10 bytes is the largest length of an encoded size
                if orig_len > 10 {
                    return Err(buck2_error!(
                        buck2_error::ErrorTag::Tier0,
                        "Corrupted stream"
                    ));
                } else {
                    return Ok(None);
                }
            }
        };

        let required_len = prost::length_delimiter_len(data_length) + data_length;
        if orig_len < required_len {
            return Ok(None);
        }
        let data = src.split_to(required_len);

        Ok(Some(data))
    }
}

#[cfg(test)]
mod tests {
    use buck2_error::BuckErrorOptionContext;
    use futures::stream::StreamExt;
    use prost::Message;
    use tokio_util::codec::FramedRead;
    use tokio_util::io::StreamReader;

    use super::*;

    #[derive(Message, PartialEq)]
    struct TestMessage {
        #[prost(string, optional, tag = "1")]
        payload: Option<String>,
    }

    #[tokio::test]
    async fn test_split() -> buck2_error::Result<()> {
        let mut buffer = Vec::new();

        let foo = TestMessage {
            payload: Some("foo".to_owned()),
        };

        let bar = TestMessage {
            payload: Some("bar".to_owned()),
        };

        foo.encode_length_delimited(&mut buffer)?;
        bar.encode_length_delimited(&mut buffer)?;

        // 1 byte at a time.
        let stream = StreamReader::new(futures::stream::iter(
            buffer
                .into_iter()
                .map(|byte| std::io::Result::Ok(BytesMut::from([byte].as_slice()))),
        ));

        let mut stream = FramedRead::new(stream, ProtobufSplitter);
        assert_eq!(
            TestMessage::decode_length_delimited(
                stream.next().await.internal_error("Missing `foo`")??
            )?,
            foo
        );
        assert_eq!(
            TestMessage::decode_length_delimited(
                stream.next().await.internal_error("Missing `bar`")??
            )?,
            bar
        );
        assert!(stream.next().await.is_none());

        Ok(())
    }
}
