/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;

use super::*;

fn encode(mappings: &[(&str, &str)], format: WriteFormat) -> HeaderMap {
    HeaderMap::from_mappings(mappings.iter().copied(), format).expect("header map should encode")
}

fn owned_mappings(map: &HeaderMap) -> Vec<(String, String)> {
    map.mappings()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.key.to_owned(), entry.value().into_owned())
        })
        .collect()
}

#[test]
fn mapping_views_borrow_strings_and_only_join_nonempty_parts() {
    for (value, prefix, suffix, borrowed) in [
        ("", "", "", true),
        ("header.h", "", "header.h", true),
        ("dir/", "dir/", "", true),
        ("dir/header.h", "dir/", "header.h", false),
    ] {
        let map = encode(&[("include.h", value)], WriteFormat::default());
        let entry = map.mappings().next().unwrap().unwrap();
        assert_eq!(entry.key, "include.h");
        assert_eq!((entry.prefix, entry.suffix), (prefix, suffix));
        let joined = entry.value();
        assert_eq!(joined, value);
        assert_eq!(matches!(joined, Cow::Borrowed(_)), borrowed);
    }
}

#[test]
fn mapping_iteration_reports_invalid_utf8_offsets() {
    let map = encode(
        &[("雪", "header.h")],
        WriteFormat::PythonHmaptool(PathStyle::Unix),
    );
    assert!(matches!(
        map.mappings().next(),
        Some(Err(HeaderMapError::InvalidUtf8(_)))
    ));
}

#[test]
fn empty_map_has_an_empty_bucket_and_reserved_string() {
    let map = encode(&[], WriteFormat::default());
    let bytes = map.to_bytes(ByteOrder::Little);
    assert_eq!(37, bytes.len());
    assert_eq!(b"pamh", &bytes[..4]);
    assert_eq!(1, u32::from_le_bytes(bytes[16..20].try_into().unwrap()));
    assert!(
        HeaderMap::from_bytes(&bytes)
            .unwrap()
            .mappings()
            .next()
            .is_none()
    );
}

#[test]
fn round_trip_both_byte_orders_and_unicode() {
    let mappings = [
        ("include/café.h", "generated/雪/naïve.hpp"),
        ("", ""),
        ("root.h", "/"),
        ("backslash.h", r"dir\file.h"),
    ];
    for signedness in [CharSignedness::Signed, CharSignedness::Unsigned] {
        let map = HeaderMap::from_mappings(
            mappings
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string())),
            WriteFormat::Clang(signedness),
        )
        .unwrap();
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let bytes = map.to_bytes(order);
            let decoded = HeaderMap::from_bytes(&bytes).unwrap();
            let actual = owned_mappings(&decoded)
                .into_iter()
                .collect::<BTreeMap<_, _>>();
            let expected = mappings
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>();
            assert_eq!(expected, actual);
            assert_eq!(bytes, decoded.to_bytes(order));
        }
    }
}

#[test]
fn clang_hash_uses_c_char_and_only_folds_ascii() {
    let signed = WriteFormat::Clang(CharSignedness::Signed);
    let unsigned = WriteFormat::Clang(CharSignedness::Unsigned);
    // UTF-8 É has bytes 195, 137, or signed chars -61, -119.
    assert_eq!(4316, hash_key("É", unsigned).unwrap());
    assert_eq!((-2340i32) as u32, hash_key("É", signed).unwrap());
    assert_ne!(
        hash_key("É", signed).unwrap(),
        hash_key("é", signed).unwrap()
    );
    assert_eq!(
        hash_key("AÉ.h", signed).unwrap(),
        hash_key("aÉ.H", signed).unwrap()
    );
    assert_eq!(3822, hash_key("abc", signed).unwrap());
}

#[test]
fn compatibility_formats_keep_their_string_layout() {
    let mappings = [("include/café.h", "generated/雪/naïve.hpp")];
    let legacy = encode(&mappings, WriteFormat::LegacyClangd);
    let python = encode(&mappings, WriteFormat::PythonHmaptool(PathStyle::Unix));
    let legacy_bucket = legacy.buckets.iter().find(|b| b[0] != 0).unwrap();
    let python_bucket = python.buckets.iter().find(|b| b[0] != 0).unwrap();
    assert_eq!(
        1 + "include/café.h".len() as u32 + 1 + "generated/雪/".len() as u32 + 1,
        legacy_bucket[2]
    );
    assert_eq!(
        1 + "include/café.h".chars().count() as u32
            + 1
            + "generated/雪/".chars().count() as u32
            + 1,
        python_bucket[2]
    );
    assert_eq!(
        "generated/雪/naïve.hpp".len() as u32,
        legacy.max_value_length
    );
    assert_eq!(
        "generated/雪/naïve.hpp".chars().count() as u32,
        python.max_value_length
    );
    assert_eq!(
        vec![(mappings[0].0.to_owned(), mappings[0].1.to_owned())],
        owned_mappings(&legacy)
    );
    assert_eq!(
        ('ä' as u32) * 13,
        hash_key("Ä", WriteFormat::PythonHmaptool(PathStyle::Unix)).unwrap()
    );
    assert!(matches!(
        hash_key("İ", WriteFormat::PythonHmaptool(PathStyle::Unix)),
        Err(HeaderMapError::InvalidLowercase('İ'))
    ));
}

#[test]
fn collisions_preserve_mapping_order() {
    let map = encode(
        &[("ab", "first.h"), ("ba", "second.h"), ("ab", "third.h")],
        WriteFormat::default(),
    );
    let start = hash_key("ab", WriteFormat::default()).unwrap() as usize & (map.buckets.len() - 1);
    for (i, expected) in ["first.h", "second.h", "third.h"].iter().enumerate() {
        let bucket = map.buckets[(start + i) & (map.buckets.len() - 1)];
        assert_eq!(*expected, map.string_at(bucket[2]).unwrap());
    }
}

#[test]
fn rejects_nul_and_unrepresentable_bucket_counts() {
    for mappings in [[("a\0b", "file.h")], [("a.h", "file\0.h")]] {
        assert!(matches!(
            HeaderMap::from_mappings(mappings.into_iter(), WriteFormat::default()),
            Err(HeaderMapError::NulByte)
        ));
    }
    let too_many = std::iter::repeat_n(("a", "b"), u32::MAX as usize);
    assert!(matches!(
        HeaderMap::from_mappings(too_many, WriteFormat::default()),
        Err(HeaderMapError::TooLarge)
    ));
}

#[test]
fn rejects_truncated_and_corrupt_files() {
    let bytes =
        encode(&[("a.h", "dir/file.h")], WriteFormat::default()).to_bytes(ByteOrder::Little);
    for end in 0..bytes.len() {
        assert!(
            HeaderMap::from_bytes(&bytes[..end]).is_err(),
            "accepted truncation at {end}"
        );
    }
    for (offset, value) in [(0, 0), (4, 2), (6, 1), (8, 0), (12, 0), (16, 3)] {
        let mut corrupt = bytes.clone();
        corrupt[offset] = value;
        assert!(
            HeaderMap::from_bytes(&corrupt).is_err(),
            "accepted corrupt field at {offset}"
        );
    }
    let mut bad_offset = bytes.clone();
    let bucket = (hash_key("a.h", WriteFormat::default()).unwrap() & 3) as usize;
    bad_offset[HEADER_SIZE + bucket * BUCKET_SIZE..HEADER_SIZE + bucket * BUCKET_SIZE + 4]
        .copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        HeaderMap::from_bytes(&bad_offset),
        Err(HeaderMapError::InvalidStringOffset(_))
    ));
    let mut bad_utf8 = bytes;
    let strings_offset = u32::from_le_bytes(bad_utf8[8..12].try_into().unwrap()) as usize;
    bad_utf8[strings_offset + 1] = 0xff;
    assert!(matches!(
        HeaderMap::from_bytes(&bad_utf8),
        Err(HeaderMapError::InvalidUtf8(_))
    ));
}

#[test]
fn rejects_full_bucket_tables() {
    let mut bytes = encode(&[("a", "b")], WriteFormat::default()).to_bytes(ByteOrder::Little);
    let filled = bytes[HEADER_SIZE..HEADER_SIZE + BUCKET_SIZE * 4]
        .as_chunks::<BUCKET_SIZE>()
        .0
        .iter()
        .find(|b| b[0] != 0)
        .unwrap()
        .to_vec();
    for bucket in bytes[HEADER_SIZE..HEADER_SIZE + BUCKET_SIZE * 4]
        .as_chunks_mut::<BUCKET_SIZE>()
        .0
    {
        bucket.copy_from_slice(&filled);
    }
    bytes[12..16].copy_from_slice(&4u32.to_le_bytes());
    assert!(matches!(
        HeaderMap::from_bytes(&bytes),
        Err(HeaderMapError::InvalidEntryCount)
    ));
}

#[test]
fn forwards_writer_errors() {
    struct BrokenWriter;
    impl Write for BrokenWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("write failed"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert!(
        encode(&[], WriteFormat::default())
            .write_to(&mut BrokenWriter, ByteOrder::Little)
            .is_err()
    );
}

#[cfg(all(buck_build, target_os = "linux"))]
#[test]
fn clang_reads_large_unicode_maps() {
    let clang = std::env::var_os("HMAP_TEST_CLANG").expect("Buck should provide Clang");
    let dir = tempfile::tempdir().unwrap();
    let header = dir.path().join("actual.h");
    std::fs::write(&header, "#define HEADER_MAP_FOUND 1\n").unwrap();
    let header = header.to_str().unwrap();
    let mut mappings = (0..400)
        .map(|i| (format!("padding/{i}.h"), header.to_owned()))
        .collect::<BTreeMap<_, _>>();
    for key in ["include/É.h", "include/雪.h"] {
        mappings.insert(key.to_owned(), header.to_owned());
    }
    let map = HeaderMap::from_mappings(
        mappings.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        WriteFormat::default(),
    )
    .unwrap();
    assert!(map.buckets.len() >= 1024);
    let source = dir.path().join("test.c");
    std::fs::write(&source, "#include <INCLUDE/É.H>\n#undef HEADER_MAP_FOUND\n#include <include/雪.h>\n#ifndef HEADER_MAP_FOUND\n#error missing header map entry\n#endif\n").unwrap();
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let hmap = dir.path().join("test.hmap");
        std::fs::write(&hmap, map.to_bytes(order)).unwrap();
        let output = std::process::Command::new(&clang)
            .args(["-cc1", "-fsyntax-only", "-I"])
            .arg(&hmap)
            .arg(&source)
            .output()
            .expect("Clang should run");
        assert!(
            output.status.success(),
            "Clang rejected {order:?} header map: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let other_signedness = match CharSignedness::native() {
        CharSignedness::Signed => CharSignedness::Unsigned,
        CharSignedness::Unsigned => CharSignedness::Signed,
    };
    let wrong_hash = HeaderMap::from_mappings(
        mappings.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        WriteFormat::Clang(other_signedness),
    )
    .unwrap();
    let hmap = dir.path().join("wrong-hash.hmap");
    std::fs::write(&hmap, wrong_hash.to_bytes(ByteOrder::Little)).unwrap();
    let output = std::process::Command::new(&clang)
        .args(["-cc1", "-fsyntax-only", "-I"])
        .arg(&hmap)
        .arg(&source)
        .output()
        .expect("Clang should run");
    assert!(
        !output.status.success(),
        "the test must detect the wrong char hash"
    );
}
