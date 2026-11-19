/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

// The code in this directory is adapted from `probminhash`
// (https://github.com/jean-pierreBoth/probminhash).

use std::hash::BuildHasher;
use std::hash::BuildHasherDefault;
use std::hash::Hash;
use std::hash::Hasher;
use std::marker::PhantomData;
use std::ops::Deref;
use std::ops::DerefMut;
use std::sync::LazyLock;

use base64::Engine;
use rand::RngExt as _;
use rand_distr::Exp1;
use rand_distr::StandardUniform;
use rand_xoshiro::Xoshiro256PlusPlus;
use rand_xoshiro::rand_core::SeedableRng;

/// Parameter set used by `SetSketcher`
#[derive(Copy, Clone, Debug)]
pub struct SetSketchParams {
    // As per the paper; q is implicit in the choice of register type
    #[cfg(test)]
    a: f64,
    b: f64,
    m: usize,
    // Derived values
    lnb: f64,
    // `1/(a*m)`
    invam: f64,
    rel_cardinality_factor: f64,
}

impl SetSketchParams {
    pub fn new(b: f64, m: usize, a: f64) -> Self {
        let lnb = (b - 1.).ln_1p();
        let rel_cardinality_factor = m as f64 * (1. - 1. / b) / (a * lnb);
        SetSketchParams {
            #[cfg(test)]
            a,
            b,
            m,
            lnb: (b - 1.).ln_1p(),
            invam: 1. / (a * (m as f64)),
            rel_cardinality_factor,
        }
    }

    pub fn recommended() -> &'static Self {
        &RECOMMENDED_PARAMS
    }

    pub fn rel_cardinality_factor(&self) -> f64 {
        self.rel_cardinality_factor
    }

    /// Returns the relative standard deviation of cardinality estimates resulting from these params
    pub fn card_estimate_std_dev(&self) -> f64 {
        let rel_std_dev = ((self.b + 1.) / (self.b - 1.) * self.lnb - 1.) / self.m as f64;
        rel_std_dev.sqrt()
    }
}

static RECOMMENDED_PARAMS: LazyLock<SetSketchParams> =
    LazyLock::new(|| SetSketchParams::new(1.001, 4096, 20.));

/// Integer type to use for registers.
type I = u16;

/// A sketch of some set.
///
/// This cannot be used to sketch any new values (but can be merged). In exchange, it doesn't
/// require a `T` or a hasher, unlike `SetSketcher`.
#[derive(Clone, Debug)]
pub struct SetSketch {
    params: &'static SetSketchParams,
    k_vec: Vec<I>,
}

impl SetSketch {
    pub fn new(params: &'static SetSketchParams) -> Self {
        let k_vec: Vec<I> = vec![0; params.m];

        SetSketch { params, k_vec }
    }

    /// Load up the sketch found in the given registers
    ///
    /// Panics if the length of the sketch doesn't match what's in the params.
    pub fn from_registers(params: &'static SetSketchParams, registers: Vec<I>) -> Self {
        assert_eq!(params.m, registers.len());
        SetSketch {
            params,
            k_vec: registers,
        }
    }

    pub fn params(&self) -> &'static SetSketchParams {
        self.params
    }

    /// The function returns an approximation of the cardinality of the sketched set.
    ///
    /// It is a relatively cpu costly function (the computed logs are not cached in the SetSketcher
    /// structure) that involves log and exp calls on the whole sketch vector.
    pub fn cardinality(&self) -> f64 {
        self.rel_cardinality() * self.params.rel_cardinality_factor
    }

    /// Returns a value that is equal to the approximate cardinality of the underlying set times a
    /// constant factor.
    ///
    /// The constant factor can be retrived from `SetSketchParams::rel_cardinality_factor`.
    pub(crate) fn rel_cardinality(&self) -> f64 {
        let sumbk = self.k_vec.iter().copied().fold(0.0f64, |acc: f64, c| {
            acc + (-(c as f64) * self.params.lnb).exp()
        });
        1. / sumbk
    }

    /// Given two sketches, returns the cardinality of the intersection of those sketches.
    ///
    /// The error of this is a constant factor in the size of the original sketches, not in the
    /// output value.
    pub fn absolute_overlap(&self, other: &Self) -> f64 {
        let self_c = self.cardinality();
        let other_c = other.cardinality();
        let merged_c = self.clone().union(other).cardinality();
        self_c + other_c - merged_c
    }

    pub fn union(mut self, other: &SetSketch) -> Self {
        self.merge(other);
        self
    }

    pub fn merge(&mut self, other: &SetSketch) {
        for i in 0..self.k_vec.len() {
            self.k_vec[i] = self.k_vec[i].max(other.k_vec[i]);
        }
    }

    pub fn get_registers(&self) -> &[I] {
        &self.k_vec
    }

    /// Computes a fast approximant of the proportion of the elements in `self` not found in
    /// `other`, returned as a confidence interval `(low, high)`.
    ///
    /// Uses only a prefix of registers, making the comparison O(1) — fast enough to run over a
    /// large candidate set as a first-pass filter before computing exact overlaps with
    /// [`SetSketch::absolute_overlap`].
    ///
    /// **Important:** This method relies on the locality-sensitive property of the sketch
    /// registers (i.e. that the register value for an element is independent of which set it
    /// belongs to). It must **not** be used with non-locality-sensitive weighting schemes.
    ///
    /// For sketches built with `sketch_weighted`, the proportions are proportions of weight
    /// mass rather than of element count.
    pub fn approx_proportion_not_included(&self, other: &SetSketch) -> (f64, f64) {
        // Implementation notes:
        //
        // Elements across the two sets fall into three categories: unique to `self` (proportion
        // `p_a`), unique to `other` (`p_b`), or in the intersection (`p_i`), with
        // `p_a + p_b + p_i = 1`.
        //
        // For each register, the stored value is the max hash over all sketched elements.
        // Because hashes are independent, the "winning" element falls into each subset with
        // probability proportional to its size:
        //   `Pr(S > O) = p_a,  Pr(S < O) = p_b,  Pr(S == O) = p_i`
        // where S and O are register values from `self` and `other` respectively.
        //
        // Counting `S > O` and `S == O` occurrences over a register prefix gives a Bernoulli
        // estimate of `p_a` and `p_i`. The desired return value is `p_a / (p_a + p_i)`,
        // wrapped in a Wilson confidence interval.
        const PREFIX_SIZE: usize = 48;

        let mut p_a = 0usize;
        let mut p_i = 0usize;
        for i in 0..PREFIX_SIZE {
            if self.k_vec[i] == other.k_vec[i] {
                p_i += 1;
            }
            if self.k_vec[i] > other.k_vec[i] {
                p_a += 1;
            }
        }
        static CONF_INTERVALS: LazyLock<[[(f64, f64); PREFIX_SIZE + 1]; PREFIX_SIZE + 1]> =
            LazyLock::new(|| {
                std::array::from_fn(|i| std::array::from_fn(|j| wilson_confidence_interval(i, j)))
            });
        CONF_INTERVALS[p_a][p_i]
    }

    /// Decode a base64-encoded sketch string into a `SetSketch`.
    ///
    /// The string is expected to be a base64-encoded sequence of native-endian
    /// `u16` register values. Both padded and unpadded base64 are accepted.
    pub fn from_base64(base64_str: &str) -> Result<Self, SetSketchDecodeError> {
        let params = SetSketchParams::recommended();
        let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(base64_str.trim())
            .map_err(SetSketchDecodeError::Base64)?;

        if bytes.len() % 2 != 0 {
            return Err(SetSketchDecodeError::OddByteCount(bytes.len()));
        }

        let registers: Vec<I> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| I::from_ne_bytes(*chunk))
            .collect();

        if registers.len() != params.m {
            return Err(SetSketchDecodeError::WrongRegisterCount {
                expected: params.m,
                actual: registers.len(),
            });
        }

        Ok(Self {
            params,
            k_vec: registers,
        })
    }

    /// Decode a versioned, base64-encoded sketch string into a `SetSketch`.
    ///
    /// The input format is `"<version>:<base64_data>"` (e.g. `"v1:AAAA..."`).  The version
    /// prefix is returned alongside the decoded sketch so callers can act on it if needed.
    pub fn from_base64_versioned(
        versioned_str: &str,
    ) -> Result<(&str, Self), SetSketchDecodeError> {
        let (version, base64_str) = versioned_str
            .split_once(':')
            .ok_or(SetSketchDecodeError::MissingVersionPrefix)?;
        let sketch = Self::from_base64(base64_str)?;
        Ok((version, sketch))
    }

    /// Like [`from_base64_versioned`](Self::from_base64_versioned), but discards the version
    /// prefix and returns only the decoded sketch.
    pub fn decode_base64_versioned(versioned_str: &str) -> Result<Self, SetSketchDecodeError> {
        let (_version, sketch) = Self::from_base64_versioned(versioned_str)?;
        Ok(sketch)
    }

    /// Decode a base64-encoded sketch that may or may not have a version prefix.
    ///
    /// If the string contains a `':'`, the portion before it is treated as a version prefix
    /// and stripped.  Otherwise the entire string is decoded as raw base64.
    pub fn from_base64_maybe_versioned(s: &str) -> Result<Self, SetSketchDecodeError> {
        match s.split_once(':') {
            Some((_version, base64_str)) => Self::from_base64(base64_str),
            None => Self::from_base64(s),
        }
    }

    /// Convenience constructor that pairs [`from_registers`](Self::from_registers) with
    /// [`SetSketchParams::recommended`].
    pub fn from_recommended_registers(registers: Vec<I>) -> Self {
        Self::from_registers(SetSketchParams::recommended(), registers)
    }

    /// Returns the cardinality estimate as an `i64`, truncating toward zero.
    pub fn get_size_approx(&self) -> i64 {
        self.cardinality() as i64
    }
}

/// Errors that can occur when decoding a `SetSketch` from base64.
#[derive(Debug)]
pub enum SetSketchDecodeError {
    Base64(base64::DecodeError),
    OddByteCount(usize),
    WrongRegisterCount { expected: usize, actual: usize },
    MissingVersionPrefix,
}

impl std::fmt::Display for SetSketchDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Base64(e) => write!(f, "base64 decode error: {}", e),
            Self::OddByteCount(n) => {
                write!(f, "invalid sketch data: byte count {} must be even", n)
            }
            Self::WrongRegisterCount { expected, actual } => {
                write!(
                    f,
                    "wrong number of registers: expected {}, got {}",
                    expected, actual
                )
            }
            Self::MissingVersionPrefix => {
                write!(f, "expected versioned format \"<version>:<base64_data>\"")
            }
        }
    }
}

impl std::error::Error for SetSketchDecodeError {}

/// For a set of Bernoulli samples with `s` successes and `f` failures, returns a confidence
/// interval for the underlying Bernoulli parameter.
///
/// This uses the Wilson confidence interval which is ~good enough most of the time; importantly it
/// does return reasonable results for `s==0`, `f==0`, or small `s+f`.
fn wilson_confidence_interval(s: usize, f: usize) -> (f64, f64) {
    // Desired z-score; 1.96 gives a 95% confidence interval
    const Z: f64 = 1.96;

    let s = s as f64;
    let f = f as f64;
    let n = s + f;
    if n == 0. {
        return (0.0, 1.0);
    }

    let zsq = Z * Z;
    let mid = (s + zsq / 2.) / (n + zsq);
    let wid = Z / (n + zsq) * (s * f / n + zsq / 4.).sqrt();
    (mid - wid, mid + wid)
}

impl Default for SetSketch {
    fn default() -> Self {
        Self::new(SetSketchParams::recommended())
    }
}

/// This structure implements Setsketch1 algorithm
///
/// The default parameters ensure capacity to represent a set up to 10^28 elements.
pub struct SetSketcher<T, H: Hasher + Default> {
    data: SetSketch,
    lower_k: f64,
    nbmin: usize,
    /// the Hasher to use if data arrive unhashed. Anyway the data type we sketch must satisfy the
    /// trait Hash
    b_hasher: BuildHasherDefault<H>,
    /// just to mark the type we sketch
    t_marker: PhantomData<T>,
}

impl<T, H: Hasher + Default> Deref for SetSketcher<T, H> {
    type Target = SetSketch;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T, H: Hasher + Default> DerefMut for SetSketcher<T, H> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

impl<T, H> SetSketcher<T, H>
where
    T: Hash,
    H: Hasher + Default,
{
    pub fn new(params: &'static SetSketchParams, b_hasher: BuildHasherDefault<H>) -> Self {
        SetSketcher::<T, H> {
            data: SetSketch::new(params),
            lower_k: 0.,
            nbmin: 0,
            b_hasher,
            t_marker: PhantomData,
        }
    }

    pub fn into_sketch(self) -> SetSketch {
        self.data
    }

    /// Add the given value to the sketch.
    ///
    /// This is just `sketch_weighted` with weight 1.
    ///
    /// Amortized `O(1)` runtime.
    pub fn sketch(&mut self, to_sketch: &T) {
        self.sketch_weighted(to_sketch, 1);
    }

    /// Just pulls out the logic that takes a sample from an exponential distribution with parameter
    /// `a` and transforms it into the value we actually store in the register.
    fn exp_sample_into_register_value(s: f64, params: &'static SetSketchParams) -> I {
        let lb_s = s.ln() / params.lnb;
        let z = (1. - lb_s).floor();
        z.clamp(0.0, I::MAX as f64) as I
    }
}

/// SplitMix64-style finalizer. This is just used as a cheap way to mix two values that we'll then
/// use to initialize a PRNG.
fn mix_octave_seed(hval: u64, octave: u32) -> u64 {
    let mut z = hval ^ 0x9E3779B97F4A7C15u64.wrapping_mul(octave as u64 + 1);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

impl<T, H> SetSketcher<T, H>
where
    T: Hash,
    H: Hasher + Default,
{
    /// Perform a weighted sketch of an item, with a slightly unusual API.
    ///
    /// The contract for the output is the strongest form of weighted sketching you could want:
    /// Inserting a new item with weight W is indistinguishable from inserting W different items of
    /// weight 1, both in terms of the distribution of a single sketch, and also jointly across
    /// sketches. So concretely, two sketches in which the same item was sketched with weight W vs
    /// W+1, the difference in the distribution of registers is as-if a single item of weight 1 had
    /// been added.
    ///
    /// In exchange for that, the caller must strictly provide for two pre-conditions:
    ///  1. `0 < weight <= max_weight`, unsurprisingly
    ///  2.  A given item (identified by the seed) may only ever be used with a single `max_weight`
    ///      parameter; if you mix `max_weight`s for a single item, all bets are off.
    ///
    /// Furthermore, the runtime of this function is `O(max_weight / (weight + cardinality))`, where
    /// cardinality is the existing cardinality of the sketch. Since usually you don't control the
    /// weight, that means you must generally ensure that the max_weight is not more than a constant
    /// factor times the existing cardinality.
    fn dart_sketch_weighted(&mut self, seed: u64, weight: f64, max_weight: f64) {
        debug_assert!(weight > 0. && weight <= max_weight);
        // We'll cover some of the basics of how setsketch works and then talk about the changes
        // that this implementation makes to support weights. As a reminder, as a starting point,
        // sketching an item I works by sampling from a particular exponential distribution (seeded
        // by I) and then computing the minimum of such samples among all items, transforming  via
        // `exp_sample_into_register_value` above, and storing the result in the register - all of
        // that once for each register, independently.
        //
        // In general, the way to support inserting a weighted item `I` with weight, say, 3, is to
        // instead insert three virtual items `I_1`, `I_2`, `I_3`. Of course, doing that naively is
        // too slow, but the strategy will be to emulate that in faster fashion.
        //
        // We are going to start by ignoring the weight vs max_weight difference and pretending we
        // are just inserting a W weighted item with weight `max_weight` (not `weight`!). We do a
        // similar optimization to the paper and flip things around a bit:
        //  1. Instead of sampling `W` times for each of the `m` registers, we do a slightly
        //     different thing of sampling `m * W` times alltogether and assigning each sample to a
        //     register uniformly at random. These samples are the "darts." Think of them as being
        //     thrown against an m by W wall. Note that this is not *exactly* the same thing, we'll
        //     fix that below.
        //  2. Then, instead of just sampling mW times, we sample directly from the distributions of
        //     "what is the minimum of those mW samples likely to be," "what is the difference
        //     between the minimum and second smallest likely to be," etc. and then reconstruct our
        //     sequence of mW samples from that. Because we end up computing minimums of everything,
        //     it is almost always the case that after a very small number of samples, the values
        //     are too big to ever matter, and we can stop.
        //  3. One consequence of this approach is that once we have a single dart that's been
        //     assigned to register 5 (for example), a later dart also assigned to register 5 cannot
        //     make a difference anymore, since we take the minimum of everything and the darts
        //     arrive in increasing order.
        //  4. As a result, instead of throwing `mW` darts, we just throw infinitely many; our only
        //     exit condition will be when the darts get too big that they can't affect anything
        //     anymore. That recovers the error from step 1 and everything is exactly correct.
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(seed);
        let scale = self.params.invam / max_weight;
        let keep_fraction = weight / max_weight;
        let mut x: f64 = 0.;
        let mut darts_since_refresh: usize = 0;
        loop {
            // It turns out all the distributions of smallest dart/difference between adjacent darts
            // in sorted order are just this.
            x += scale * rng.sample::<f64, Exp1>(Exp1);
            // This is a decreasing function; as `x` grows, this decreases.
            let k = Self::exp_sample_into_register_value(x, self.params);
            // `lower_k` is a value we cache that is a (not necessarily optimal) lower bound for the
            // values among all the registers. Since register values only increase, once `k` has
            // gotten this small no future darts can ever affect the state of the sketch again and
            // we can exit.
            if k as f64 <= self.lower_k {
                break;
            }
            // However, `lower_k` is a stale bound in general, and we need to make sure we never go
            // too long without refreshing it, otherwise we might not notice that we hit our exit
            // criteria (the check below is not enough - a test reproduces that).
            darts_since_refresh += 1;
            if darts_since_refresh >= self.params.m {
                darts_since_refresh = 0;
                self.refresh_lower_k();
                if k as f64 <= self.lower_k {
                    break;
                }
            }
            // Here is where we remember that the actual weight is not `max_weight` but rather
            // `weight`: We need the important property that for two weights `w' > w`, the set of
            // darts that we include for `w'` is a strict superset of the set of darts that we
            // include for `w`, otherwise all of our marginal behavior is wrong.
            //
            // So in addition to assigning each dart a random register, we assign it a logical
            // "height" chosen uniformly at random between `0` and `1` at which it appears and only
            // consider darts that appear at height at most `weight / max_weight` of the way up; the
            // height assignments are made independently of the value of `weight`, so they are
            // consistent.
            //
            // This behavior is why the weird runtime: If the `weight / max_weight` fraction is too
            // low, all of our darts end up "too high" and we never get a chance to hit our exit
            // condition.
            let height: f64 = rng.sample::<f64, _>(StandardUniform);
            let register_bits: u64 = rng.sample::<u64, _>(StandardUniform);
            if height >= keep_fraction {
                continue;
            }
            // Fixed-point map of register_bits onto 0..m (bias < m/2^64). Like
            // `mix_octave_seed`, this map is part of the persisted-sketch format.
            let register = ((register_bits as u128 * self.params.m as u128) >> 64) as usize;
            if k > self.k_vec[register] {
                self.k_vec[register] = k;
                self.nbmin += 1;
                // The refresh above only fires once one insert goes to `m` darts; however that
                // allows for an edge case in which a bunch of inserts each go to `m/2` darts before
                // exiting, this protects against that by also refreshing once at least `m` updates
                // have happened across streams.
                if self.nbmin.is_multiple_of(self.params.m) {
                    self.refresh_lower_k();
                }
            }
        }
    }

    fn refresh_lower_k(&mut self) {
        let flow = self
            .k_vec
            .iter()
            .fold(self.k_vec[0], |min: I, x| if x < &min { *x } else { min })
            as f64;
        self.lower_k = flow;
    }

    /// Add the given value to the sketch using the given weight.
    ///
    /// The cardinality estimate that results from weighted insertions is the sum of the weights.
    ///
    /// The recommended parameters are tuned for cardinality estimates between 1 and 10^28; weights
    /// that cause cardinality estimates outside of that range may be poorly behaved.
    ///
    /// When the same item is sketched more than once using different weights, the following
    /// behaviors apply:
    ///
    ///  1. Within one sketch, only the largest weight "counts"; re-inserting with smaller weights
    ///     is a nop.
    ///  2. All cardinality estimates and locality sensitivity remains fully correct.
    ///
    /// Use of this function can be mixed with the unweighted `sketch`; `sketch` is just an alias
    /// for this with weight 1.
    ///
    /// This function runs in time amortized `O(log W)`.
    ///
    /// The algorithm used here is based on BagMinHash (Ertl 2018, <https://arxiv.org/abs/1802.03914>)
    /// and DartMinHash (Christiani 2020, <https://arxiv.org/abs/2005.11547>) with some gentle
    /// massaging for compatibility with setsketch and to improve presentation.
    pub fn sketch_weighted(&mut self, to_sketch: &T, weight: u64) {
        if weight == 0 {
            return;
        }
        let hval: u64 = self.b_hasher.hash_one(&to_sketch);
        // We'd like to use `dart_sketch_weighted`, but need to do some trickery to work around the
        // odd performance characteristics of that. We do something like this: Imagine we have an
        // item with weight 21; instead of sketching that item directly, we sketch virtual items
        // with weights like `(I_1, 1), (I_2, 1), (I_4, 2), (I_8, 4), (I_16, 8), (I_32, 5)`, one
        // per octave `[2^(j-1), 2^j)` of the interval `[0, 21)`; the `max_weight` of each item is
        // the width of its octave (so it's trivially fixed per-item), and every item has full
        // weight except the one on the octave straddling 21, which retains only the part below 21.
        //
        // The insertion order is what makes sure the max_weight is never much more than the
        // cardinality: the largest *full* octave goes first, because it is guaranteed to raise the
        // sketch's cardinality to at least `weight / 4`, putting every later stream's max_weight
        // within a constant factor of the cardinality. The straddling octave cannot be trusted
        // with this job even though it has the largest max_weight: it may retain almost nothing
        // (consider weight = 2^n + 1), and unretained darts never improve registers, so on a cold
        // sketch it would run its full `O(max_weight / (weight + cardinality))` cost with nothing
        // in the denominator.
        let top = weight.ilog2();
        self.dart_octave(hval, top, weight);
        if !weight.is_power_of_two() {
            // The straddling octave enumerates at twice the rate of the largest full one, so get
            // it done while whatever mass it does retain can still benefit the remaining streams.
            self.dart_octave(hval, top + 1, weight);
        }
        for j in (0..top).rev() {
            self.dart_octave(hval, j, weight);
        }
    }

    /// Sketches the octave-`j` virtual sub-item of an item with the given hash and total weight.
    ///
    /// Octave `j` covers `[2^(j-1), 2^j)` of the mass interval (octave 0 covers `[0, 1)`); the
    /// sub-item's weight is however much of that interval lies below `weight`.
    fn dart_octave(&mut self, hval: u64, j: u32, weight: u64) {
        let lo: u128 = if j == 0 { 0 } else { 1u128 << (j - 1) };
        let hi: u128 = 1u128 << j;
        let max_weight = (hi - lo) as f64;
        let sub_weight = ((weight as u128).min(hi) - lo) as f64;
        self.dart_sketch_weighted(mix_octave_seed(hval, j), sub_weight, max_weight);
    }
}

#[cfg(test)]
mod tests {

    use fnv::FnvHasher;

    use super::*;

    fn check_cardinality_is_about(s: &SetSketch, val: u64) {
        let val = val as f64;
        let mean = s.cardinality();
        let stddev = s.params().card_estimate_std_dev();
        if (mean - val).abs() / val > 3.0 * stddev {
            panic!("Mean {} != expected {}", mean, val);
        }
    }

    fn usize_sketcher() -> SetSketcher<usize, FnvHasher> {
        SetSketcher::new(
            SetSketchParams::recommended(),
            BuildHasherDefault::<FnvHasher>::default(),
        )
    }

    #[test]
    fn test_merge_1() {
        let vbmax = 2000;
        let va: Vec<usize> = (0..1000).collect();
        let vb: Vec<usize> = (900..vbmax).collect();
        let union = 2000;

        let mut sethasher_a = usize_sketcher();
        // now compute sketches
        for va in va.iter() {
            sethasher_a.sketch(va);
        }
        let mut sethasher_b = usize_sketcher();
        // now compute sketches
        for vb in vb.iter() {
            sethasher_b.sketch(vb);
        }
        // merging vb into va
        sethasher_a.merge(&sethasher_b);
        check_cardinality_is_about(&sethasher_a, union);
    }

    #[test]
    fn check_weighted_single() {
        for v in [1, 10, 10000] {
            let mut s = usize_sketcher();
            s.sketch_weighted(&0, v);
            check_cardinality_is_about(&s, v);
        }
    }

    #[test]
    fn check_weighted_sums_reasonably() {
        let mut a = usize_sketcher();
        for i in 1..=100 {
            a.sketch_weighted(&i, i as u64);
        }
        check_cardinality_is_about(&a, 5050);
    }

    #[test]
    fn check_well_behaved_under_different_weights() {
        let mut a = usize_sketcher();
        a.sketch_weighted(&0, 100);
        for i in 1..=10 {
            a.sketch_weighted(&i, 10);
        }
        a.sketch_weighted(&0, 105);
        check_cardinality_is_about(&a, 205);
    }

    #[test]
    fn check_large_weights() {
        let mut a = usize_sketcher();
        const MULT: u64 = 1_000_000_000_000;
        for i in 1..=100 {
            a.sketch_weighted(&i, i as u64 * MULT);
        }
        check_cardinality_is_about(&a, 5050 * MULT);
    }

    #[test]
    fn check_dart_locality_and_monotonicity_on_mismatched_weights() {
        for (wa, wb) in [(100u64, 150u64), (5, 8), (5, 6), (1000, 1500), (7, 970)] {
            let mut a = usize_sketcher();
            a.sketch_weighted(&0, wa);
            let mut b = usize_sketcher();
            b.sketch_weighted(&0, wb);

            let mut matches = 0usize;
            for (ra, rb) in a.get_registers().iter().zip(b.get_registers().iter()) {
                // Exact: the smaller weight's darts are a subset of the larger's.
                assert!(ra <= rb, "w={wa} vs {wb}: register {ra} > {rb}");
                if ra == rb {
                    matches += 1;
                }
            }
            // The match probability is the weighted Jaccard similarity `wa / wb`, plus a
            // correction for both registers landing in the zero bucket (negligible under
            // the recommended params, where `Pr[register = 0] = exp(-w / inva)`).
            let q = wa as f64 / wb as f64;
            let p = q + (1. - q) * (-(wb as f64) * a.params.a).exp();
            let m = a.params.m as f64;
            let sigma = (m * p * (1. - p)).sqrt();
            assert!(
                ((matches as f64) - m * p).abs() < 5. * sigma + 8.,
                "w={wa} vs {wb}: {matches} matches, expected ~{}",
                m * p
            );
        }
    }

    #[test]
    fn check_dart_max_weight_wins_exactly() {
        let mut b = usize_sketcher();
        b.sketch_weighted(&7, 5);

        let mut a = usize_sketcher();
        a.sketch_weighted(&7, 5);
        a.sketch_weighted(&7, 3);
        assert_eq!(a.get_registers(), b.get_registers());

        let mut c = usize_sketcher();
        c.sketch_weighted(&7, 3);
        c.sketch_weighted(&7, 5);
        assert_eq!(c.get_registers(), b.get_registers());
    }

    #[test]
    fn check_dart_weighted_jaccard_multi_element() {
        // A: 0..100 at weight 2 (mass 200). B: 50..150 at weight 3 (mass 300).
        // sum max = 50*2 + 50*3 + 50*3 = 400; per register:
        //   Pr[S == O] = sum min / sum max     = 100/400
        //   Pr[S > O]  = sum (wA - wB)+ / max  = 100/400
        //   Pr[S < O]  =                         200/400
        let mut a = usize_sketcher();
        for v in 0..100usize {
            a.sketch_weighted(&v, 2);
        }
        let mut b = usize_sketcher();
        for v in 50..150usize {
            b.sketch_weighted(&v, 3);
        }
        check_cardinality_is_about(&a, 200);
        check_cardinality_is_about(&b, 300);
        check_cardinality_is_about(&a.clone().union(&b), 400);

        let (mut eq, mut gt, mut lt) = (0usize, 0usize, 0usize);
        for (ra, rb) in a.get_registers().iter().zip(b.get_registers().iter()) {
            match ra.cmp(rb) {
                std::cmp::Ordering::Equal => eq += 1,
                std::cmp::Ordering::Greater => gt += 1,
                std::cmp::Ordering::Less => lt += 1,
            }
        }
        let m = a.params.m as f64;
        for (count, p, name) in [(eq, 0.25, "eq"), (gt, 0.25, "gt"), (lt, 0.5, "lt")] {
            let sigma = (m * p * (1. - p)).sqrt();
            assert!(
                ((count as f64) - m * p).abs() < 5. * sigma + 8.,
                "{name}: {count} vs expected ~{}",
                m * p
            );
        }

        // The O(1) prefix filter reads weight-mass proportions:
        // p_a / (p_a + p_i) = 0.25 / 0.5 = 0.5, up to 48-register prefix noise. Bounds kept
        // deliberately loose.
        let (low, high) = a.approx_proportion_not_included(&b);
        assert!(low < 0.62, "low bound {low}");
        assert!(high > 0.38, "high bound {high}");
    }

    #[test]
    fn check_dart_heavy_weight_into_partially_filled_sketch() {
        // Regression canary for `lower_k` staleness: without the per-stream refresh in
        // `dart_sketch_weighted` this enumerates ~a*m*w (order 1e17) darts and effectively
        // hangs.
        let mut a = usize_sketcher();
        for v in 0..3usize {
            a.sketch_weighted(&v, 1);
        }
        a.sketch_weighted(&1000, 1_000_000_000_000);
        check_cardinality_is_about(&a, 1_000_000_000_003);
    }

    #[test]
    fn check_dart_cut_octave_into_cold_sketch() {
        // Cost canary for the octave insertion order: the first octave processed must be a
        // full one. Weight 2^40 + 1 cuts octave 41 down to weight 1; processed against an
        // empty sketch, that octave would enumerate ~a*m*2^40 darts (years of work), since
        // its darts are almost never retained and so can never raise `lower_k` themselves.
        let mut a = usize_sketcher();
        a.sketch_weighted(&0, (1 << 40) + 1);
        check_cardinality_is_about(&a, (1 << 40) + 1);
    }

    #[test]
    fn check_dart_nesting_law_directly() {
        // The primitive's contract in isolation: same (seed, max_weight), different
        // weight. Match probability is 2/8 plus the zero-bucket correction (negligible
        // under the recommended params).
        let seed = 0x5EED_u64;
        let mut full = usize_sketcher();
        full.dart_sketch_weighted(seed, 8., 8.);
        let mut thin = usize_sketcher();
        thin.dart_sketch_weighted(seed, 2., 8.);

        check_cardinality_is_about(&full, 8);
        check_cardinality_is_about(&thin, 2);

        let mut matches = 0usize;
        for (rt, rf) in thin.get_registers().iter().zip(full.get_registers().iter()) {
            assert!(rt <= rf);
            if rt == rf {
                matches += 1;
            }
        }
        let p = 0.25 + 0.75 * (-8.0f64 * full.params.a).exp();
        let m = full.params.m as f64;
        let sigma = (m * p * (1. - p)).sqrt();
        assert!(
            ((matches as f64) - m * p).abs() < 5. * sigma + 8.,
            "{matches} matches, expected ~{}",
            m * p
        );
    }

    #[test]
    fn test_approx_proportion_not_included() {
        let a_vals: Vec<usize> = (0..100).collect();
        let b_vals: Vec<usize> = (70..150).collect();

        let mut a = usize_sketcher();
        for v in &a_vals {
            a.sketch(v);
        }
        let mut b = usize_sketcher();
        for v in &b_vals {
            b.sketch(v);
        }

        check_cardinality_is_about(&a, 100);
        check_cardinality_is_about(&b, 80);

        let merged = a.clone().union(&b);
        check_cardinality_is_about(&merged, 150);

        let (low, high) = a.approx_proportion_not_included(&b);
        assert!(low > 0.5, "low bound {low} should be > 0.5");
        assert!(high < 0.9, "high bound {high} should be < 0.9");
    }
}
