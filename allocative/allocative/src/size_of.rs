/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::Allocative;
use crate::FlameGraphBuilder;
use crate::Key;
use crate::Visitor;
use crate::visitor::NodeKind;
use crate::visitor::VisitorImpl;

/// Size of a value and all allocations reachable from it.
///
/// Shared allocations are counted once per pointer identity, even when they are reachable through
/// multiple fields. This has the same accounting semantics as [`FlameGraphBuilder`], without
/// exposing the generated flamegraph to the caller.
pub fn size_of_reachable(root: &dyn Allocative) -> usize {
    let mut builder = FlameGraphBuilder::default();
    builder.visit_root(root);
    builder.finish().flamegraph().total_size()
}

/// Size of data allocated in unique pointers in the struct.
///
/// * Exclude self
/// * Exclude shared pointers
/// * For unique pointers, include the size of the pointee plus this function recursively
///
/// # Example
///
/// ```
/// use allocative::Allocative;
///
/// #[derive(Allocative)]
/// struct Foo {
///     data: Vec<u8>,
/// }
///
/// assert_eq!(
///     3,
///     allocative::size_of_unique_allocated_data(&Foo {
///         data: vec![10, 20, 30]
///     })
/// );
/// ```
pub fn size_of_unique_allocated_data(root: &dyn Allocative) -> usize {
    struct SizeOfUniqueAllocatedDataVisitor {
        /// Size we return.
        size: usize,
    }

    impl VisitorImpl for SizeOfUniqueAllocatedDataVisitor {
        fn enter_inline_impl(&mut self, _name: Key, size: usize, parent: NodeKind) {
            if let NodeKind::Unique = parent {
                self.size += size;
            }
        }

        fn enter_unique_impl(&mut self, _name: Key, _size: usize, _parent: NodeKind) {}

        fn enter_shared_impl(
            &mut self,
            _name: Key,
            _size: usize,
            _ptr: *const (),
            _parent: NodeKind,
        ) -> bool {
            false
        }

        fn exit_inline_impl(&mut self) {}

        fn exit_unique_impl(&mut self) {}

        fn exit_shared_impl(&mut self) {
            unreachable!("shared pointers are not visited")
        }

        fn exit_root_impl(&mut self) {}
    }

    let mut visitor_impl = SizeOfUniqueAllocatedDataVisitor { size: 0 };
    let mut visitor = Visitor {
        visitor: &mut visitor_impl,
        node_kind: NodeKind::Root,
    };
    root.visit(&mut visitor);
    visitor.exit();
    visitor_impl.size
}

/// Size of a piece of data and data allocated in unique pointers in the struct.
///
/// * Excludes shared pointers
///
/// # Example
///
/// ```
/// use allocative::Allocative;
///
/// #[derive(Allocative)]
/// struct Foo {
///     data: Vec<u8>,
/// }
///
/// assert_eq!(
///     3 + std::mem::size_of::<Vec<u8>>(),
///     allocative::size_of_unique(&Foo {
///         data: vec![10, 20, 30]
///     })
/// );
/// ```
pub fn size_of_unique<T>(root: &T) -> usize
where
    T: Allocative,
{
    std::mem::size_of::<T>() + size_of_unique_allocated_data(root)
}

#[cfg(test)]
mod tests {
    use std::mem;
    use std::sync::Arc;

    use allocative_derive::Allocative;

    use crate as allocative;
    use crate::size_of_reachable;
    use crate::size_of_unique;
    use crate::size_of_unique_allocated_data;

    #[test]
    fn test_size_of_reachable_counts_shared_allocation_once() {
        #[derive(Allocative)]
        #[expect(dead_code)]
        struct SingleArc {
            value: Arc<Vec<u8>>,
        }

        #[derive(Allocative)]
        #[expect(dead_code)]
        struct DuplicateArc {
            first: Arc<Vec<u8>>,
            second: Arc<Vec<u8>>,
        }

        let value = Arc::new(vec![0; 128]);
        let shared_allocation_bytes =
            2 * mem::size_of::<usize>() + mem::size_of::<Vec<u8>>() + value.capacity();
        let single = SingleArc {
            value: value.clone(),
        };
        let duplicate = DuplicateArc {
            first: value.clone(),
            second: value,
        };

        let single_bytes = size_of_reachable(&single);
        let duplicate_bytes = size_of_reachable(&duplicate);

        assert_eq!(
            mem::size_of::<SingleArc>() + shared_allocation_bytes,
            single_bytes,
        );
        assert_eq!(
            mem::size_of::<DuplicateArc>() + shared_allocation_bytes,
            duplicate_bytes,
        );
        assert_eq!(
            single_bytes - mem::size_of::<SingleArc>(),
            duplicate_bytes - mem::size_of::<DuplicateArc>(),
        );
    }

    #[test]
    fn test_box() {
        #[derive(Allocative)]
        #[expect(dead_code)]
        struct Boxed {
            data: Box<u32>,
        }

        let boxed = Boxed { data: Box::new(17) };

        assert_eq!(mem::size_of::<u32>(), size_of_unique_allocated_data(&boxed));

        assert_eq!(
            mem::size_of::<u32>() + mem::size_of::<Boxed>(),
            size_of_unique(&boxed)
        );
    }

    #[test]
    fn test_box_slice() {
        #[derive(Allocative)]
        #[expect(dead_code)]
        struct Boxed {
            data: Box<[u32]>,
        }

        let boxed = Boxed {
            data: vec![1, 2, 3].into_boxed_slice(),
        };

        assert_eq!(
            mem::size_of::<u32>() * 3,
            size_of_unique_allocated_data(&boxed)
        );

        assert_eq!(
            mem::size_of::<Boxed>() + mem::size_of::<u32>() * 3,
            size_of_unique(&boxed)
        );
    }

    #[test]
    fn test_struct_in_box() {
        #[derive(Allocative)]
        #[expect(dead_code)]
        struct Data {
            a: u8,
            b: Box<u32>,
        }

        #[derive(Allocative)]
        #[expect(dead_code)]
        struct Boxed {
            data: Box<Data>,
        }

        let boxed = Boxed {
            data: Box::new(Data {
                a: 1,
                b: Box::new(2),
            }),
        };

        assert_eq!(
            mem::size_of::<Data>() + mem::size_of::<u32>(),
            size_of_unique_allocated_data(&boxed)
        );

        assert_eq!(
            mem::size_of::<Boxed>() + mem::size_of::<Data>() + mem::size_of::<u32>(),
            size_of_unique(&boxed)
        );
    }
}
