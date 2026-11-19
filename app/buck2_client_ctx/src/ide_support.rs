/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::conversion::from_any_with_tag;
use bytes::BytesMut;
use futures::Stream;
use futures::StreamExt;
use serde::Deserialize;
use serde::Serialize;
use tokio::io::AsyncRead;
use tokio_util::codec::Decoder;
use tokio_util::codec::FramedRead;

/// Reads from input a stream of lsp-like messages and returns them as serde_json serialized strings.
pub fn ide_message_stream<T: AsyncRead, Message: for<'a> Deserialize<'a> + Serialize>(
    input: T,
) -> impl Stream<Item = buck2_error::Result<String>> {
    FramedRead::new(
        input,
        LspMessageLikeDecoder::<Message> {
            _marker: std::marker::PhantomData,
        },
    )
    .map(|m| m.and_then(|m| Ok(serde_json::to_string(&m)?)))
}

pub struct LspMessageLikeDecoder<T: for<'a> Deserialize<'a>> {
    _marker: std::marker::PhantomData<T>,
}

impl<T: for<'a> Deserialize<'a>> Decoder for LspMessageLikeDecoder<T> {
    type Item = T;
    type Error = buck2_error::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // The LSP (and DAP) protocol allows at most 2 headers (Content-Length and Content-Type), but since a
        // header is 2 pointers we allow ourselves quite a few more.
        let mut headers_buff = [httparse::EMPTY_HEADER; 16];

        let (headers_length, headers) = match httparse::parse_headers(src, &mut headers_buff)
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))
            .buck_error_context("Invalid headers")?
        {
            httparse::Status::Complete(r) => r,
            httparse::Status::Partial => return Ok(None),
        };

        let mut content_length: Option<usize> = None;

        for h in headers {
            if h.name.eq_ignore_ascii_case("Content-Length") {
                content_length = Some(
                    std::str::from_utf8(h.value)
                        .buck_error_context("Content-Length is not utf-8")?
                        .parse()
                        .buck_error_context("Content-Length is not a number")?,
                );
                break;
            }
        }

        let content_length = content_length.internal_error("Content-Length is missing")?;

        if src.len() < headers_length + content_length {
            return Ok(None);
        }

        let _headers = src.split_to(headers_length);
        let text = src.split_to(content_length);
        Some(serde_json::from_slice(&text).buck_error_context("Invalid request")).transpose()
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use lsp_server::Message;
    use lsp_server::Request;
    use lsp_server::RequestId;

    use super::*;

    #[test]
    fn test_decoder() -> buck2_error::Result<()> {
        let r1 = Request {
            id: RequestId::from(1i32),
            method: "m1".into(),
            params: "foobar".into(),
        };

        let r2 = Request {
            id: RequestId::from(2i32),
            method: "m2".into(),
            params: "barbaz".into(),
        };

        let mut bytes = BytesMut::new();

        {
            // This wants a `Write` and BytesMut doesn't do that so just use a Vec.
            let mut tmp = Vec::new();
            Message::Request(r1.clone()).write(&mut tmp)?;
            Message::Request(r2.clone()).write(&mut tmp)?;
            bytes.extend_from_slice(&tmp);
        };

        let mut decoder = LspMessageLikeDecoder {
            _marker: std::marker::PhantomData,
        };

        // Decoding a subset should return None and not consume anything.
        {
            let mut tmp = BytesMut::new();
            tmp.extend_from_slice(&bytes[0..8]);
            assert_matches!(decoder.decode(&mut tmp)?, None);
            assert_eq!(tmp.len(), 8);
        }

        assert_matches!(decoder.decode(&mut bytes)?, Some(Message::Request(Request {
            id, method, params
        })) => {
            assert_eq!(id, r1.id);
            assert_eq!(method, r1.method);
            assert_eq!(params, r1.params);
        });

        assert_matches!(decoder.decode(&mut bytes)?, Some(Message::Request(Request {
            id, method, params
        })) => {
            assert_eq!(id, r2.id);
            assert_eq!(method, r2.method);
            assert_eq!(params, r2.params);
        });

        assert_matches!(decoder.decode(&mut bytes)?, None);
        assert_eq!(bytes.len(), 0);

        Ok(())
    }
}
