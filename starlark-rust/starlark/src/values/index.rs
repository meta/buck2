/*
 * Copyright 2018 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use crate::values::ValueError;

/// One bound of a slice as a position in the sequence: `default` if the bound was left out,
/// and otherwise the bound, counted from the end if it is negative, brought into `min..=max`.
fn convert_slice_bound(len: i32, bound: Option<i32>, default: i32, min: i32, max: i32) -> i32 {
    match bound {
        None => default,
        Some(x) => {
            let i = if x < 0 { len + x } else { x };
            if i < min {
                min
            } else if i > max {
                max
            } else {
                i
            }
        }
    }
}

/// The position `x` names in a sequence of length `len`, counted from the end if `x` is
/// negative; the out-of-bounds error if there is none. For `at` and `set_at`.
pub(crate) fn convert_index(x: i32, len: i32) -> crate::Result<i32> {
    let i = if x < 0 {
        len.checked_add(x).ok_or(ValueError::IntegerOverflow)?
    } else {
        x
    };
    if i < 0 || i >= len {
        Err(ValueError::IndexOutOfBound(i).into())
    } else {
        Ok(i)
    }
}

/// The bounds and the stride of `xs[start:stop:stride]` for a sequence of length `len`, each
/// bound as a position in the sequence. An operand that is `None` was left out or is `None`.
/// The error if the stride is 0.
pub(crate) fn convert_slice_indices(
    len: i32,
    start: Option<i32>,
    stop: Option<i32>,
    stride: Option<i32>,
) -> crate::Result<(i32, i32, i32)> {
    let stride = stride.unwrap_or(1);
    if stride == 0 {
        return Err(ValueError::IndexOutOfBound(0).into());
    }
    let def_start = if stride < 0 { len - 1 } else { 0 };
    let def_end = if stride < 0 { -1 } else { len };
    let clamp = if stride < 0 { -1 } else { 0 };
    Ok((
        convert_slice_bound(len, start, def_start, clamp, len + clamp),
        convert_slice_bound(len, stop, def_end, clamp, len + clamp),
        stride,
    ))
}

/// `xs[start:stop:stride]`. An operand that is `None` was left out or is `None`.
pub(crate) fn apply_slice<T: Copy>(
    xs: &[T],
    start: Option<i32>,
    stop: Option<i32>,
    stride: Option<i32>,
) -> crate::Result<Vec<T>> {
    let (start, stop, stride) = convert_slice_indices(xs.len() as i32, start, stop, stride)?;
    if stride == 1 {
        if start >= stop {
            return Ok(Vec::new());
        } else {
            return Ok(xs[start as usize..stop as usize].to_vec());
        }
    }

    let (start, stop) = if stride < 0 {
        (stop + 1, start + 1)
    } else {
        (start, stop)
    };
    if start >= stop {
        return Ok(Vec::new());
    }
    let mut res = xs[start as usize..stop as usize].to_vec();
    if stride == -1 {
        res.reverse();
        return Ok(res);
    }
    if stride < 0 {
        res.reverse();
    }
    let astride = stride.unsigned_abs();
    let res = res
        .into_iter()
        .enumerate()
        .filter_map(|x| {
            if (x.0 as u32).is_multiple_of(astride) {
                Some(x.1)
            } else {
                None
            }
        })
        .collect();
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_index() {
        assert_eq!(Some(6), convert_index(6, 7).ok());
        assert_eq!(Some(6), convert_index(-1, 7).ok());
        assert_eq!(
            Some((6, 7, 1)),
            convert_slice_indices(7, Some(6), None, None).ok()
        );
        assert_eq!(
            Some((6, -1, -1)),
            convert_slice_indices(7, Some(-1), None, Some(-1)).ok()
        );
        assert_eq!(
            Some((6, 7, 1)),
            convert_slice_indices(7, Some(-1), Some(10), None).ok()
        );
        // Errors
        assert!(convert_index(8, 7).is_err()); // 8 > 7 = len
        assert!(convert_index(-8, 7).is_err()); // -8 + 7 = -1 < 0
    }

    #[test]
    fn test_apply_slice() {
        let s = &[0, 1, 2, 3, 4, 5, 6];

        let x = apply_slice(s, Some(-1), None, Some(-1)).unwrap();
        assert_eq!(x, &[6, 5, 4, 3, 2, 1, 0]);

        let x = apply_slice(s, Some(-1), None, Some(-1)).unwrap();
        assert_eq!(x, &[6, 5, 4, 3, 2, 1, 0]);

        let x = apply_slice(s, Some(0), Some(3), Some(2)).unwrap();
        assert_eq!(x, &[0, 2]);

        let x = apply_slice(s, Some(5), Some(2), Some(-2)).unwrap();
        assert_eq!(x, &[5, 3]);

        let x = apply_slice(s, Some(-1), Some(-5), Some(-1)).unwrap();
        assert_eq!(x, &[6, 5, 4, 3]);

        let x = apply_slice(s, Some(-1), Some(0), Some(-1)).unwrap();
        assert_eq!(x, &[6, 5, 4, 3, 2, 1]);

        let x = apply_slice(&[1, 2, 3], Some(0), Some(-2), Some(-1)).unwrap();
        assert_eq!(x, &[] as &[i32]);
    }

    #[test]
    fn test_slice_i32_min_stride_no_panic() {
        // Regression for https://github.com/meta/starlark-rust/issues/230
        // i32::MIN stride previously panicked on abs()
        let xs = [1, 2, 3];
        let res = apply_slice(&xs, None, None, Some(i32::MIN)).unwrap();
        assert_eq!(res, vec![3]);
        let xs2 = ['a', 'b', 'c', 'd', 'e'];
        let res2 = apply_slice(&xs2, None, None, Some(i32::MIN)).unwrap();
        assert_eq!(res2, vec!['e']);
        // neighbour -2147483647 works too
        let res3 = apply_slice(&xs, None, None, Some(-2147483647)).unwrap();
        assert_eq!(res3, vec![3]);
    }
}
