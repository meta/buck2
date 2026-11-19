/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Borrow;
use std::fmt::Debug;
use std::fmt::Display;
use std::hash::Hash;
use std::ops::Deref;
use std::str;

use allocative::Allocative;
use dupe::Dupe;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use pagable::arc_erase::ArcErase;
use pagable::arc_erase::ArcEraseType;
use pagable::arc_erase::ArcSerializeOutcome;
use pagable::arc_erase::StdArcEraseType;
use pagable::arc_erase::deserialize_arc;
use serde::Deserialize;
use serde::Serialize;
use strong_hash::StrongHash;

use crate::arc_str::base::ArcStrBase;
use crate::arc_str::base::ArcStrBaseInner;
use crate::arc_str::base::ArcStrLenStrategy;
use crate::size_assert;

struct ThinArcStrProperties;

unsafe impl ArcStrLenStrategy for ThinArcStrProperties {
    type AllocatedPayload = u32;
    type ValuePayload = ();
    const EMPTY: (Self::AllocatedPayload, Self::ValuePayload) = (0, ());

    #[inline]
    fn unpack_len(d: (Self::AllocatedPayload, Self::ValuePayload)) -> u32 {
        d.0
    }

    #[inline]
    fn pack_len(len: u32) -> (Self::AllocatedPayload, Self::ValuePayload) {
        (len, ())
    }

    #[inline]
    fn inner_empty() -> &'static ArcStrBaseInner<Self> {
        &INNER_EMPTY
    }
}

/// Wrapper for `Arc<str>`.
#[derive(
    PartialEq, Eq, Hash, StrongHash, PartialOrd, Ord, Allocative, Clone, Dupe, Default
)]
pub struct ThinArcStr {
    base: ArcStrBase<ThinArcStrProperties>,
}

impl ArcErase for ThinArcStr {
    type Weak = ();

    fn dupe_strong(&self) -> Self {
        self.dupe()
    }

    fn erase_type() -> impl ArcEraseType {
        StdArcEraseType::<Self>::new()
    }

    fn identity(&self) -> usize {
        self.base.addr()
    }

    fn downgrade(&self) -> Option<Self::Weak> {
        None
    }

    fn is_unique(&self) -> bool {
        self.base.is_unique()
    }

    fn serialize_inner(
        &self,
        ser: &mut dyn PagableSerializer,
    ) -> pagable::Result<ArcSerializeOutcome> {
        Serialize::serialize(&self, ser.serde())?;
        Ok(ArcSerializeOutcome::Serialized)
    }

    fn deserialize_inner<'de, D: PagableDeserializer<'de> + ?Sized>(
        deser: &mut D,
    ) -> pagable::Result<Self> {
        Ok(Self::deserialize(deser.serde())?)
    }
}

impl PagableSerialize for ThinArcStr {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        serializer.serialize_arc(self)
    }
}

impl<'de> PagableDeserialize<'de> for ThinArcStr {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        deserialize_arc::<Self, _>(deserializer)
    }
}

size_assert::words_of_type!(ThinArcStr, 1);
size_assert::words_of_type!(Option<ThinArcStr>, 1);

static INNER_EMPTY: ArcStrBaseInner<ThinArcStrProperties> = ArcStrBaseInner::empty();

impl ThinArcStr {
    #[inline]
    pub fn as_str(&self) -> &str {
        self.base.as_str()
    }
}

impl Deref for ThinArcStr {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for ThinArcStr {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for ThinArcStr {
    #[inline]
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl Display for ThinArcStr {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self.as_str(), f)
    }
}

impl Debug for ThinArcStr {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(self.as_str(), f)
    }
}

impl<'a> From<&'a str> for ThinArcStr {
    #[inline]
    fn from(s: &'a str) -> ThinArcStr {
        ThinArcStr {
            base: ArcStrBase::from(s),
        }
    }
}

impl From<String> for ThinArcStr {
    #[inline]
    fn from(s: String) -> ThinArcStr {
        ThinArcStr::from(s.as_str())
    }
}

impl Serialize for ThinArcStr {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_str().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ThinArcStr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(String::deserialize(deserializer)?.into())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hash;
    use std::hash::Hasher;
    use std::sync::atomic;

    use dupe::Dupe;

    use crate::arc_str::thin::ThinArcStr;

    #[test]
    fn test_serialize() {
        assert_eq!(
            "\"hello\"",
            serde_json::to_string(&ThinArcStr::from("hello")).unwrap()
        );
        assert_eq!(
            "\"\"",
            serde_json::to_string(&ThinArcStr::from("")).unwrap()
        );
    }

    #[test]
    fn test_hash() {
        fn hash<H: Hash + ?Sized>(h: &H) -> u64 {
            let mut hasher = DefaultHasher::new();
            h.hash(&mut hasher);
            hasher.finish()
        }

        assert_eq!(hash(""), hash(&ThinArcStr::from("")));
        assert_eq!(hash("hello"), hash(&ThinArcStr::from("hello")));
    }

    #[test]
    fn test_clone_drop_ref_count() {
        fn ref_count(s: &ThinArcStr) -> u32 {
            assert!(!s.is_empty());
            s.base.inner().refcount.load(atomic::Ordering::Relaxed)
        }

        let s = ThinArcStr::from("hello");
        assert_eq!(1, ref_count(&s));
        let s2 = s.dupe();
        assert_eq!(2, ref_count(&s));
        drop(s);
        assert_eq!(1, ref_count(&s2));
    }

    #[test]
    fn test_little_stress_test() {
        // Interpreted, a string costs milliseconds on the Windows CI hosts.
        let (rounds, churn) = if cfg!(miri) { (20, 100) } else { (100, 1000) };
        for i in 0..rounds {
            let strings = (0..i)
                .map(|i| ThinArcStr::from(i.to_string()))
                .collect::<Vec<_>>();
            for j in 0..churn {
                let _ignore = ThinArcStr::from(j.to_string());
            }
            for (k, s) in strings.iter().enumerate() {
                assert_eq!(k.to_string(), s.as_str());
            }
        }
    }

    // Re-runs the test binary as a subprocess, which Miri cannot.
    #[cfg(all(unix, not(miri)))]
    mod allocation_failure {
        use std::alloc::GlobalAlloc;
        use std::alloc::Layout;
        use std::alloc::System;
        use std::ffi::CStr;
        use std::mem;
        use std::os::unix::process::ExitStatusExt;
        use std::process::Command;

        use crate::arc_str::base::ArcStrBaseInner;
        use crate::arc_str::thin::ThinArcStr;
        use crate::arc_str::thin::ThinArcStrProperties;

        const LEN: usize = 1_234_567;
        const HELPER_ENV: &CStr = c"BUCK2_UTIL_ARC_STR_ALLOCATION_FAILURE_HELPER";

        /// The size of the heap block behind a `LEN`-byte `ThinArcStr`.
        fn failing_size() -> usize {
            mem::size_of::<ArcStrBaseInner<ThinArcStrProperties>>() + LEN
        }

        /// `System`, except that in the helper subprocess an allocation of exactly
        /// `failing_size()` bytes fails. The decision reads only the layout and the environment:
        /// the optimizer assumes that allocator calls do not touch program memory, so a flag in
        /// a static would not be reliable here.
        struct FailStringAllocation;

        unsafe impl GlobalAlloc for FailStringAllocation {
            unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
                if layout.size() == failing_size()
                    && !unsafe { libc::getenv(HELPER_ENV.as_ptr()) }.is_null()
                {
                    return std::ptr::null_mut();
                }
                unsafe { System.alloc(layout) }
            }

            unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
                unsafe { System.dealloc(ptr, layout) }
            }
        }

        #[global_allocator]
        static GLOBAL: FailStringAllocation = FailStringAllocation;

        /// Does the work only in the subprocess spawned by `is_reported`.
        #[test]
        fn helper() {
            if std::env::var_os(HELPER_ENV.to_str().unwrap()).is_none() {
                return;
            }
            let s = "a".repeat(LEN);
            std::hint::black_box(ThinArcStr::from(s.as_str()));
        }

        #[test]
        fn is_reported() {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "arc_str::thin::tests::allocation_failure::helper",
                    "--nocapture",
                ])
                .env(HELPER_ENV.to_str().unwrap(), "1")
                .output()
                .unwrap();
            // The test harness may run the helper in a child of its own and report the child's
            // fate in its own output, so look at everything.
            let report = format!(
                "{:?}\n{}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            // The failed allocation is reported by `handle_alloc_error`, which aborts.
            assert!(
                report.contains(&format!(
                    "memory allocation of {} bytes failed",
                    failing_size()
                )),
                "{report}"
            );
            assert!(
                output.status.signal() != Some(libc::SIGSEGV)
                    && !report.contains("signal 11")
                    && !report.contains("SIGSEGV"),
                "{report}"
            );
        }
    }
}
