//-
// Copyright 2017, 2018 The proptest developers
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::std_facade::{fmt, Arc, Box, Rc};
use core::cmp;

use crate::strategy::*;
use crate::test_runner::*;

//==============================================================================
// Traits
//==============================================================================

/// A new [`ValueTree`] from a [`Strategy`] when [`Ok`] or otherwise [`Err`]
/// when a new value-tree can not be produced for some reason such as
/// in the case of filtering with a predicate which always returns false.
/// You should pass in your strategy as the type parameter.
///
/// [`Strategy`]: trait.Strategy.html
/// [`ValueTree`]: trait.ValueTree.html
/// [`Ok`]: https://doc.rust-lang.org/nightly/std/result/enum.Result.html#variant.Ok
/// [`Err`]: https://doc.rust-lang.org/nightly/std/result/enum.Result.html#variant.Err
pub type NewTree<S> = Result<<S as Strategy>::Tree, Reason>;

/// A strategy for producing arbitrary values of a given type.
///
/// `fmt::Debug` is a hard requirement for all strategies currently due to
/// `prop_flat_map()`. This constraint will be removed when specialisation
/// becomes stable.
#[must_use = "strategies do nothing unless used"]
pub trait Strategy: fmt::Debug {
    /// The value tree generated by this `Strategy`.
    type Tree: ValueTree<Value = Self::Value>;

    /// The type of value used by functions under test generated by this Strategy.
    ///
    /// This corresponds to the same type as the associated type `Value`
    /// in `Self::Tree`. It is provided here to simplify usage particularly
    /// in conjunction with `-> impl Strategy<Value = MyType>`.
    type Value: fmt::Debug;

    /// Generate a new value tree from the given runner.
    ///
    /// This may fail if there are constraints on the generated value and the
    /// generator is unable to produce anything that satisfies them. Any
    /// failure is wrapped in `TestError::Abort`.
    ///
    /// This method is generally expected to be deterministic. That is, given a
    /// `TestRunner` with its RNG in a particular state, this should produce an
    /// identical `ValueTree` every time. Non-deterministic strategies do not
    /// cause problems during normal operation, but they do break failure
    /// persistence since it is implemented by simply saving the seed used to
    /// generate the test case.
    fn new_tree(&self, runner: &mut TestRunner) -> NewTree<Self>;

    /// Returns a strategy which produces values transformed by the function
    /// `fun`.
    ///
    /// There is no need (or possibility, for that matter) to define how the
    /// output is to be shrunken. Shrinking continues to take place in terms of
    /// the source value.
    ///
    /// `fun` should be a deterministic function. That is, for a given input
    /// value, it should produce an equivalent output value on every call.
    /// Proptest assumes that it can call the function as many times as needed
    /// to generate as many identical values as needed. For this reason, `F` is
    /// `Fn` rather than `FnMut`.
    fn prop_map<O: fmt::Debug, F: Fn(Self::Value) -> O>(
        self,
        fun: F,
    ) -> Map<Self, F>
    where
        Self: Sized,
    {
        Map {
            source: self,
            fun: Arc::new(fun),
        }
    }

    /// Returns a strategy which produces values of type `O` by transforming
    /// `Self` with `Into<O>`.
    ///
    /// You should always prefer this operation instead of `prop_map` when
    /// you can as it is both clearer and also currently more efficient.
    ///
    /// There is no need (or possibility, for that matter) to define how the
    /// output is to be shrunken. Shrinking continues to take place in terms of
    /// the source value.
    fn prop_map_into<O: fmt::Debug>(self) -> MapInto<Self, O>
    where
        Self: Sized,
        Self::Value: Into<O>,
    {
        MapInto::new(self)
    }

    /// Returns a strategy which produces values transformed by the function
    /// `fun`, which is additionally given a random number generator.
    ///
    /// This is exactly like `prop_map()` except for the addition of the second
    /// argument to the function. This allows introducing chaotic variations to
    /// generated values that are not easily expressed otherwise while allowing
    /// shrinking to proceed reasonably.
    ///
    /// During shrinking, `fun` is always called with an identical random
    /// number generator, so if it is a pure function it will always perform
    /// the same perturbation.
    ///
    /// ## Example
    ///
    /// ```
    /// // The prelude also gets us the `Rng` trait.
    /// use proptest::prelude::*;
    ///
    /// proptest! {
    ///   #[test]
    ///   fn test_something(a in (0i32..10).prop_perturb(
    ///       // Perturb the integer `a` (range 0..10) to a pair of that
    ///       // integer and another that's ± 10 of it.
    ///       // Note that this particular case would be better implemented as
    ///       // `(0i32..10, -10i32..10).prop_map(|(a, b)| (a, a + b))`
    ///       // but is shown here for simplicity.
    ///       |centre, rng| (centre, centre + rng.gen_range(-10, 10))))
    ///   {
    ///       // Test stuff
    ///   }
    /// }
    /// # fn main() { }
    /// ```
    fn prop_perturb<O: fmt::Debug, F: Fn(Self::Value, TestRng) -> O>(
        self,
        fun: F,
    ) -> Perturb<Self, F>
    where
        Self: Sized,
    {
        Perturb {
            source: self,
            fun: Arc::new(fun),
        }
    }

    /// Maps values produced by this strategy into new strategies and picks
    /// values from those strategies.
    ///
    /// `fun` is used to transform the values produced by this strategy into
    /// other strategies. Values are then chosen from the derived strategies.
    /// Shrinking proceeds by shrinking individual values as well as shrinking
    /// the input used to generate the internal strategies.
    ///
    /// ## Shrinking
    ///
    /// In the case of test failure, shrinking will not only shrink the output
    /// from the combinator itself, but also the input, i.e., the strategy used
    /// to generate the output itself. Doing this requires searching the new
    /// derived strategy for a new failing input. The combinator will generate
    /// up to `Config::cases` values for this search.
    ///
    /// As a result, nested `prop_flat_map`/`Flatten` combinators risk
    /// exponential run time on this search for new failing values. To ensure
    /// that test failures occur within a reasonable amount of time, all of
    /// these combinators share a single "flat map regen" counter, and will
    /// stop generating new values if it exceeds `Config::max_flat_map_regens`.
    ///
    /// ## Example
    ///
    /// Generate two integers, where the second is always less than the first,
    /// without using filtering:
    ///
    /// ```
    /// use proptest::prelude::*;
    ///
    /// proptest! {
    ///   # /*
    ///   #[test]
    ///   # */
    ///   fn test_two(
    ///     // Pick integers in the 1..65536 range, and derive a strategy
    ///     // which emits a tuple of that integer and another one which is
    ///     // some value less than it.
    ///     (a, b) in (1..65536).prop_flat_map(|a| (Just(a), 0..a))
    ///   ) {
    ///     prop_assert!(b < a);
    ///   }
    /// }
    /// #
    /// # fn main() { test_two(); }
    /// ```
    ///
    /// ## Choosing the right flat-map
    ///
    /// `Strategy` has three "flat-map" combinators. They look very similar at
    /// first, and can be used to produce superficially identical test results.
    /// For example, the following three expressions all produce inputs which
    /// are 2-tuples `(a,b)` where the `b` component is less than `a`.
    ///
    /// ```no_run
    /// # #![allow(unused_variables)]
    /// use proptest::prelude::*;
    ///
    /// let flat_map = (1..10).prop_flat_map(|a| (Just(a), 0..a));
    /// let ind_flat_map = (1..10).prop_ind_flat_map(|a| (Just(a), 0..a));
    /// let ind_flat_map2 = (1..10).prop_ind_flat_map2(|a| 0..a);
    /// ```
    ///
    /// The three do differ however in terms of how they shrink.
    ///
    /// For `flat_map`, both `a` and `b` will shrink, and the invariant that
    /// `b < a` is maintained. This is a "dependent" or "higher-order" strategy
    /// in that it remembers that the strategy for choosing `b` is dependent on
    /// the value chosen for `a`.
    ///
    /// For `ind_flat_map`, the invariant `b < a` is maintained, but only
    /// because `a` does not shrink. This is due to the fact that the
    /// dependency between the strategies is not tracked; `a` is simply seen as
    /// a constant.
    ///
    /// Finally, for `ind_flat_map2`, the invariant `b < a` is _not_
    /// maintained, because `a` can shrink independently of `b`, again because
    /// the dependency between the two variables is not tracked, but in this
    /// case the derivation of `a` is still exposed to the shrinking system.
    ///
    /// The use-cases for the independent flat-map variants is pretty narrow.
    /// For the majority of cases where invariants need to be maintained and
    /// you want all components to shrink, `prop_flat_map` is the way to go.
    /// `prop_ind_flat_map` makes the most sense when the input to the map
    /// function is not exposed in the output and shrinking across strategies
    /// is not expected to be useful. `prop_ind_flat_map2` is useful for using
    /// related values as starting points while not constraining them to that
    /// relation.
    fn prop_flat_map<S: Strategy, F: Fn(Self::Value) -> S>(
        self,
        fun: F,
    ) -> Flatten<Map<Self, F>>
    where
        Self: Sized,
    {
        Flatten::new(Map {
            source: self,
            fun: Arc::new(fun),
        })
    }

    /// Maps values produced by this strategy into new strategies and picks
    /// values from those strategies while considering the new strategies to be
    /// independent.
    ///
    /// This is very similar to `prop_flat_map()`, but shrinking will *not*
    /// attempt to shrink the input that produces the derived strategies. This
    /// is appropriate for when the derived strategies already fully shrink in
    /// the desired way.
    ///
    /// In most cases, you want `prop_flat_map()`.
    ///
    /// See `prop_flat_map()` for a more detailed explanation on how the
    /// three flat-map combinators differ.
    fn prop_ind_flat_map<S: Strategy, F: Fn(Self::Value) -> S>(
        self,
        fun: F,
    ) -> IndFlatten<Map<Self, F>>
    where
        Self: Sized,
    {
        IndFlatten(Map {
            source: self,
            fun: Arc::new(fun),
        })
    }

    /// Similar to `prop_ind_flat_map()`, but produces 2-tuples with the input
    /// generated from `self` in slot 0 and the derived strategy in slot 1.
    ///
    /// See `prop_flat_map()` for a more detailed explanation on how the
    /// three flat-map combinators differ differ.
    fn prop_ind_flat_map2<S: Strategy, F: Fn(Self::Value) -> S>(
        self,
        fun: F,
    ) -> IndFlattenMap<Self, F>
    where
        Self: Sized,
    {
        IndFlattenMap {
            source: self,
            fun: Arc::new(fun),
        }
    }

    /// Returns a strategy which only produces values accepted by `fun`.
    ///
    /// This results in a very naïve form of rejection sampling and should only
    /// be used if (a) relatively few values will actually be rejected; (b) it
    /// isn't easy to express what you want by using another strategy and/or
    /// `map()`.
    ///
    /// There are a lot of downsides to this form of filtering. It slows
    /// testing down, since values must be generated but then discarded.
    /// Proptest only allows a limited number of rejects this way (across the
    /// entire `TestRunner`). Rejection can interfere with shrinking;
    /// particularly, complex filters may largely or entirely prevent shrinking
    /// from substantially altering the original value.
    ///
    /// Local rejection sampling is still preferable to rejecting the entire
    /// input to a test (via `TestCaseError::Reject`), however, and the default
    /// number of local rejections allowed is much higher than the number of
    /// whole-input rejections.
    ///
    /// `whence` is used to record where and why the rejection occurred.
    fn prop_filter<R: Into<Reason>, F: Fn(&Self::Value) -> bool>(
        self,
        whence: R,
        fun: F,
    ) -> Filter<Self, F>
    where
        Self: Sized,
    {
        Filter::new(self, whence.into(), fun)
    }

    /// Returns a strategy which only produces transformed values where `fun`
    /// returns `Some(value)` and rejects those where `fun` returns `None`.
    ///
    /// Using this method is preferable to using `.prop_map(..).prop_filter(..)`.
    ///
    /// This results in a very naïve form of rejection sampling and should only
    /// be used if (a) relatively few values will actually be rejected; (b) it
    /// isn't easy to express what you want by using another strategy and/or
    /// `map()`.
    ///
    /// There are a lot of downsides to this form of filtering. It slows
    /// testing down, since values must be generated but then discarded.
    /// Proptest only allows a limited number of rejects this way (across the
    /// entire `TestRunner`). Rejection can interfere with shrinking;
    /// particularly, complex filters may largely or entirely prevent shrinking
    /// from substantially altering the original value.
    ///
    /// Local rejection sampling is still preferable to rejecting the entire
    /// input to a test (via `TestCaseError::Reject`), however, and the default
    /// number of local rejections allowed is much higher than the number of
    /// whole-input rejections.
    ///
    /// `whence` is used to record where and why the rejection occurred.
    fn prop_filter_map<F: Fn(Self::Value) -> Option<O>, O: fmt::Debug>(
        self,
        whence: impl Into<Reason>,
        fun: F,
    ) -> FilterMap<Self, F>
    where
        Self: Sized,
    {
        FilterMap::new(self, whence.into(), fun)
    }

    /// Returns a strategy which picks uniformly from `self` and `other`.
    ///
    /// When shrinking, if a value from `other` was originally chosen but that
    /// value can be shrunken no further, it switches to a value from `self`
    /// and starts shrinking that.
    ///
    /// Be aware that chaining `prop_union` calls will result in a very
    /// right-skewed distribution. If this is not what you want, you can call
    /// the `.or()` method on the `Union` to add more values to the same union,
    /// or directly call `Union::new()`.
    ///
    /// Both `self` and `other` must be of the same type. To combine
    /// heterogeneous strategies, call the `boxed()` method on both `self` and
    /// `other` to erase the type differences before calling `prop_union()`.
    fn prop_union(self, other: Self) -> Union<Self>
    where
        Self: Sized,
    {
        Union::new(vec![self, other])
    }

    /// Generate a recursive structure with `self` items as leaves.
    ///
    /// `recurse` is applied to various strategies that produce the same type
    /// as `self` with nesting depth _n_ to create a strategy that produces the
    /// same type with nesting depth _n+1_. Generated structures will have a
    /// depth between 0 and `depth` and will usually have up to `desired_size`
    /// total elements, though they may have more. `expected_branch_size` gives
    /// the expected maximum size for any collection which may contain
    /// recursive elements and is used to control branch probability to achieve
    /// the desired size. Passing a too small value can result in trees vastly
    /// larger than desired.
    ///
    /// Note that `depth` only counts branches; i.e., `depth = 0` is a single
    /// leaf, and `depth = 1` is a leaf or a branch containing only leaves.
    ///
    /// In practise, generated values usually have a lower depth than `depth`
    /// (but `depth` is a hard limit) and almost always under
    /// `expected_branch_size` (though it is not a hard limit) since the
    /// underlying code underestimates probabilities.
    ///
    /// Shrinking shrinks both the inner values and attempts switching from
    /// recursive to non-recursive cases.
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// # #![allow(unused_variables)]
    /// use std::collections::HashMap;
    ///
    /// use proptest::prelude::*;
    ///
    /// /// Define our own JSON AST type
    /// #[derive(Debug, Clone)]
    /// enum JsonNode {
    ///   Null,
    ///   Bool(bool),
    ///   Number(f64),
    ///   String(String),
    ///   Array(Vec<JsonNode>),
    ///   Map(HashMap<String, JsonNode>),
    /// }
    ///
    /// # fn main() {
    /// #
    /// // Define a strategy for generating leaf nodes of the AST
    /// let json_leaf = prop_oneof![
    ///   Just(JsonNode::Null),
    ///   prop::bool::ANY.prop_map(JsonNode::Bool),
    ///   prop::num::f64::ANY.prop_map(JsonNode::Number),
    ///   ".*".prop_map(JsonNode::String),
    /// ];
    ///
    /// // Now define a strategy for a whole tree
    /// let json_tree = json_leaf.prop_recursive(
    ///   4, // No more than 4 branch levels deep
    ///   64, // Target around 64 total elements
    ///   16, // Each collection is up to 16 elements long
    ///   |element| prop_oneof![
    ///     // NB `element` is an `Arc` and we'll need to reference it twice,
    ///     // so we clone it the first time.
    ///     prop::collection::vec(element.clone(), 0..16)
    ///       .prop_map(JsonNode::Array),
    ///     prop::collection::hash_map(".*", element, 0..16)
    ///       .prop_map(JsonNode::Map)
    ///   ]);
    /// # }
    /// ```
    fn prop_recursive<
        R: Strategy<Value = Self::Value> + 'static,
        F: Fn(BoxedStrategy<Self::Value>) -> R,
    >(
        self,
        depth: u32,
        desired_size: u32,
        expected_branch_size: u32,
        recurse: F,
    ) -> Recursive<Self::Value, F>
    where
        Self: Sized + 'static,
    {
        Recursive::new(self, depth, desired_size, expected_branch_size, recurse)
    }

    /// Shuffle the contents of the values produced by this strategy.
    ///
    /// That is, this modifies a strategy producing a `Vec`, slice, etc, to
    /// shuffle the contents of that `Vec`/slice/etc.
    ///
    /// Initially, the value is fully shuffled. During shrinking, the input
    /// value will initially be unchanged while the result will gradually be
    /// restored to its original order. Once de-shuffling either completes or
    /// is cancelled by calls to `complicate()` pinning it to a particular
    /// permutation, the inner value will be simplified.
    ///
    /// ## Example
    ///
    /// ```
    /// use proptest::prelude::*;
    ///
    /// static VALUES: &'static [u32] = &[0, 1, 2, 3, 4];
    ///
    /// fn is_permutation(orig: &[u32], mut actual: Vec<u32>) -> bool {
    ///   actual.sort();
    ///   orig == &actual[..]
    /// }
    ///
    /// proptest! {
    ///   # /*
    ///   #[test]
    ///   # */
    ///   fn test_is_permutation(
    ///       ref perm in Just(VALUES.to_owned()).prop_shuffle()
    ///   ) {
    ///       assert!(is_permutation(VALUES, perm.clone()));
    ///   }
    /// }
    /// #
    /// # fn main() { test_is_permutation(); }
    /// ```
    fn prop_shuffle(self) -> Shuffle<Self>
    where
        Self: Sized,
        Self::Value: Shuffleable,
    {
        Shuffle(self)
    }

    /// Erases the type of this `Strategy` so it can be passed around as a
    /// simple trait object.
    ///
    /// See also `sboxed()` if this `Strategy` is `Send` and `Sync` and you
    /// want to preserve that information.
    ///
    /// Strategies of this type afford cheap shallow cloning via reference
    /// counting by using an `Arc` internally.
    fn boxed(self) -> BoxedStrategy<Self::Value>
    where
        Self: Sized + 'static,
    {
        BoxedStrategy(Arc::new(BoxedStrategyWrapper(self)))
    }

    /// Erases the type of this `Strategy` so it can be passed around as a
    /// simple trait object.
    ///
    /// Unlike `boxed()`, this conversion retains the `Send` and `Sync` traits
    /// on the output.
    ///
    /// Strategies of this type afford cheap shallow cloning via reference
    /// counting by using an `Arc` internally.
    fn sboxed(self) -> SBoxedStrategy<Self::Value>
    where
        Self: Sized + Send + Sync + 'static,
    {
        SBoxedStrategy(Arc::new(BoxedStrategyWrapper(self)))
    }

    /// Wraps this strategy to prevent values from being subject to shrinking.
    ///
    /// Suppressing shrinking is useful when testing things like linear
    /// approximation functions. Ordinarily, proptest will tend to shrink the
    /// input to the function until the result is just barely outside the
    /// acceptable range whereas the original input may have produced a result
    /// far outside of it. Since this makes it harder to see what the actual
    /// problem is, making the input `NoShrink` allows learning about inputs
    /// that produce more incorrect results.
    fn no_shrink(self) -> NoShrink<Self>
    where
        Self: Sized,
    {
        NoShrink(self)
    }
}

/// A generated value and its associated shrinker.
///
/// Conceptually, a `ValueTree` represents a spectrum between a "minimally
/// complex" value and a starting, randomly-chosen value. For values such as
/// numbers, this can be thought of as a simple binary search, and this is how
/// the `ValueTree` state machine is defined.
///
/// The `ValueTree` state machine notionally has three fields: low, current,
/// and high. Initially, low is the "minimally complex" value for the type, and
/// high and current are both the initially chosen value. It can be queried for
/// its current state. When shrinking, the controlling code tries simplifying
/// the value one step. If the test failure still happens with the simplified
/// value, further simplification occurs. Otherwise, the code steps back up
/// towards the prior complexity.
///
/// The main invariants here are that the "high" value always corresponds to a
/// failing test case, and that repeated calls to `complicate()` will return
/// `false` only once the "current" value has returned to what it was before
/// the last call to `simplify()`.
///
/// While it would be possible for default do-nothing implementations of
/// `simplify()` and `complicate()` to be provided, this was not done
/// deliberately since the majority of strategies will want to define their own
/// shrinking anyway, and the minority that do not must call it out explicitly
/// by their own implementation.
pub trait ValueTree {
    /// The type of the value produced by this `ValueTree`.
    type Value: fmt::Debug;

    /// Returns the current value.
    fn current(&self) -> Self::Value;
    /// Attempts to simplify the current value. Notionally, this sets the
    /// "high" value to the current value, and the current value to a "halfway
    /// point" between high and low, rounding towards low.
    ///
    /// Returns whether any state changed as a result of this call. This does
    /// not necessarily imply that the value of `current()` has changed, since
    /// in the most general case, it is not possible for an implementation to
    /// determine this.
    ///
    /// This call needs to correctly handle being called even immediately after
    /// it had been called previously and returned `false`.
    fn simplify(&mut self) -> bool;
    /// Attempts to partially undo the last simplification. Notionally, this
    /// sets the "low" value to one plus the current value, and the current
    /// value to a "halfway point" between high and the new low, rounding
    /// towards low.
    ///
    /// Returns whether any state changed as a result of this call. This does
    /// not necessarily imply that the value of `current()` has changed, since
    /// in the most general case, it is not possible for an implementation to
    /// determine this.
    ///
    /// It is usually expected that, immediately after a call to `simplify()`
    /// which returns true, this call will itself return true. However, this is
    /// not always the case; in some strategies, particularly those that use
    /// some form of rejection sampling, the act of trying to simplify may
    /// change the state such that `simplify()` returns true, yet ultimately
    /// left the resulting value unchanged, in which case there is nothing left
    /// to complicate.
    ///
    /// This call does not need to gracefully handle being called before
    /// `simplify()` was ever called, but does need to correctly handle being
    /// called even immediately after it had been called previously and
    /// returned `false`.
    fn complicate(&mut self) -> bool;
}

//==============================================================================
// NoShrink
//==============================================================================

/// Wraps a `Strategy` or `ValueTree` to suppress shrinking of generated
/// values.
///
/// See `Strategy::no_shrink()` for more details.
#[derive(Clone, Copy, Debug)]
#[must_use = "strategies do nothing unless used"]
pub struct NoShrink<T>(T);

impl<T: Strategy> Strategy for NoShrink<T> {
    type Tree = NoShrink<T::Tree>;
    type Value = T::Value;

    fn new_tree(&self, runner: &mut TestRunner) -> NewTree<Self> {
        self.0.new_tree(runner).map(NoShrink)
    }
}

impl<T: ValueTree> ValueTree for NoShrink<T> {
    type Value = T::Value;

    fn current(&self) -> T::Value {
        self.0.current()
    }

    fn simplify(&mut self) -> bool {
        false
    }
    fn complicate(&mut self) -> bool {
        false
    }
}

//==============================================================================
// Trait objects
//==============================================================================

macro_rules! proxy_strategy {
    ($typ:ty $(, $lt:tt)*) => {
        impl<$($lt,)* S : Strategy + ?Sized> Strategy for $typ {
            type Tree = S::Tree;
            type Value = S::Value;

            fn new_tree(&self, runner: &mut TestRunner) -> NewTree<Self> {
                (**self).new_tree(runner)
            }
        }
    };
}
proxy_strategy!(Box<S>);
proxy_strategy!(&'a S, 'a);
proxy_strategy!(&'a mut S, 'a);
proxy_strategy!(Rc<S>);
proxy_strategy!(Arc<S>);

impl<T: ValueTree + ?Sized> ValueTree for Box<T> {
    type Value = T::Value;
    fn current(&self) -> Self::Value {
        (**self).current()
    }
    fn simplify(&mut self) -> bool {
        (**self).simplify()
    }
    fn complicate(&mut self) -> bool {
        (**self).complicate()
    }
}

/// A boxed `ValueTree`.
type BoxedVT<T> = Box<dyn ValueTree<Value = T>>;

/// A boxed `Strategy` trait object as produced by `Strategy::boxed()`.
///
/// Strategies of this type afford cheap shallow cloning via reference
/// counting by using an `Arc` internally.
#[derive(Debug)]
#[must_use = "strategies do nothing unless used"]
pub struct BoxedStrategy<T>(Arc<dyn Strategy<Value = T, Tree = BoxedVT<T>>>);

/// A boxed `Strategy` trait object which is also `Sync` and
/// `Send`, as produced by `Strategy::sboxed()`.
///
/// Strategies of this type afford cheap shallow cloning via reference
/// counting by using an `Arc` internally.
#[derive(Debug)]
#[must_use = "strategies do nothing unless used"]
pub struct SBoxedStrategy<T>(
    Arc<dyn Strategy<Value = T, Tree = BoxedVT<T>> + Sync + Send>,
);

impl<T> Clone for BoxedStrategy<T> {
    fn clone(&self) -> Self {
        BoxedStrategy(Arc::clone(&self.0))
    }
}

impl<T> Clone for SBoxedStrategy<T> {
    fn clone(&self) -> Self {
        SBoxedStrategy(Arc::clone(&self.0))
    }
}

impl<T: fmt::Debug> Strategy for BoxedStrategy<T> {
    type Tree = BoxedVT<T>;
    type Value = T;

    fn new_tree(&self, runner: &mut TestRunner) -> NewTree<Self> {
        self.0.new_tree(runner)
    }

    // Optimization: Don't rebox the strategy.

    fn boxed(self) -> BoxedStrategy<Self::Value>
    where
        Self: Sized + 'static,
    {
        self
    }
}

impl<T: fmt::Debug> Strategy for SBoxedStrategy<T> {
    type Tree = BoxedVT<T>;
    type Value = T;

    fn new_tree(&self, runner: &mut TestRunner) -> NewTree<Self> {
        self.0.new_tree(runner)
    }

    // Optimization: Don't rebox the strategy.

    fn sboxed(self) -> SBoxedStrategy<Self::Value>
    where
        Self: Sized + Send + Sync + 'static,
    {
        self
    }

    fn boxed(self) -> BoxedStrategy<Self::Value>
    where
        Self: Sized + 'static,
    {
        BoxedStrategy(self.0)
    }
}

#[derive(Debug)]
struct BoxedStrategyWrapper<T>(T);
impl<T: Strategy> Strategy for BoxedStrategyWrapper<T>
where
    T::Tree: 'static,
{
    type Tree = Box<dyn ValueTree<Value = T::Value>>;
    type Value = T::Value;

    fn new_tree(&self, runner: &mut TestRunner) -> NewTree<Self> {
        Ok(Box::new(self.0.new_tree(runner)?))
    }
}

//==============================================================================
// Sanity checking
//==============================================================================

/// Options passed to `check_strategy_sanity()`.
#[derive(Clone, Copy, Debug)]
pub struct CheckStrategySanityOptions {
    /// If true (the default), require that `complicate()` return `true` at
    /// least once after any call to `simplify()` which itself returns once.
    ///
    /// This property is not required by contract, but many strategies are
    /// designed in a way that this is expected to hold.
    pub strict_complicate_after_simplify: bool,

    /// If true, cause local rejects to return an error instead of retrying.
    /// Defaults to false. Useful for testing behaviors around error handling.
    pub error_on_local_rejects: bool,

    // Needs to be public for FRU syntax.
    #[allow(missing_docs)]
    #[doc(hidden)]
    pub _non_exhaustive: (),
}

impl Default for CheckStrategySanityOptions {
    fn default() -> Self {
        CheckStrategySanityOptions {
            strict_complicate_after_simplify: true,
            error_on_local_rejects: false,
            _non_exhaustive: (),
        }
    }
}

/// Run some tests on the given `Strategy` to ensure that it upholds the
/// simplify/complicate contracts.
///
/// This is used to internally test proptest, but is made generally available
/// for external implementations to use as well.
///
/// `options` can be passed to configure the test; if `None`, the defaults are
/// used. Note that the defaults check for certain properties which are **not**
/// actually required by the `Strategy` and `ValueTree` contracts; if you think
/// your code is right but it fails the test, consider whether a non-default
/// configuration is necessary.
///
/// This can work with fallible strategies, but limits how many times it will
/// retry failures.
pub fn check_strategy_sanity<S: Strategy>(
    strategy: S,
    options: Option<CheckStrategySanityOptions>,
) where
    S::Tree: Clone + fmt::Debug,
    S::Value: cmp::PartialEq,
{
    // Like assert_eq!, but also pass if both values do not equal themselves.
    // This allows the test to work correctly with things like NaN.
    macro_rules! assert_same {
        ($a:expr, $b:expr, $($stuff:tt)*) => { {
            let a = $a;
            let b = $b;
            if a == a || b == b {
                assert_eq!(a, b, $($stuff)*);
            }
        } }
    }

    let options = options.unwrap_or_else(CheckStrategySanityOptions::default);
    let mut config = Config::default();
    if options.error_on_local_rejects {
        config.max_local_rejects = 0;
    }
    let mut runner = TestRunner::new(config);

    for _ in 0..1024 {
        let mut gen_tries = 0;
        let mut state;
        loop {
            let err = match strategy.new_tree(&mut runner) {
                Ok(s) => {
                    state = s;
                    break;
                }
                Err(e) => e,
            };

            gen_tries += 1;
            if gen_tries > 100 {
                panic!(
                    "Strategy passed to check_strategy_sanity failed \
                     to generate a value over 100 times in a row; \
                     last failure reason: {}",
                    err
                );
            }
        }

        {
            let mut state = state.clone();
            let mut count = 0;
            while state.simplify() || state.complicate() {
                count += 1;
                if count > 65536 {
                    panic!(
                        "Failed to converge on any value. State:\n{:#?}",
                        state
                    );
                }
            }
        }

        let mut num_simplifies = 0;
        let mut before_simplified;
        loop {
            before_simplified = state.clone();
            if !state.simplify() {
                break;
            }

            let mut complicated = state.clone();
            let before_complicated = state.clone();
            if options.strict_complicate_after_simplify {
                assert!(
                    complicated.complicate(),
                    "complicate() returned false immediately after \
                     simplify() returned true. internal state after \
                     {} calls to simplify():\n\
                     {:#?}\n\
                     simplified to:\n\
                     {:#?}\n\
                     complicated to:\n\
                     {:#?}",
                    num_simplifies,
                    before_simplified,
                    state,
                    complicated
                );
            }

            let mut prev_complicated = complicated.clone();
            let mut num_complications = 0;
            loop {
                if !complicated.complicate() {
                    break;
                }
                prev_complicated = complicated.clone();
                num_complications += 1;

                if num_complications > 65_536 {
                    panic!(
                        "complicate() returned true over 65536 times in a \
                         row; aborting due to possible infinite loop. \
                         If this is not an infinite loop, it may be \
                         necessary to reconsider how shrinking is \
                         implemented or use a simpler test strategy. \
                         Internal state:\n{:#?}",
                        state
                    );
                }
            }

            assert_same!(
                before_simplified.current(),
                complicated.current(),
                "Calling simplify(), then complicate() until it \
                 returned false, did not return to the value before \
                 simplify. Expected:\n\
                 {:#?}\n\
                 Actual:\n\
                 {:#?}\n\
                 Internal state after {} calls to simplify():\n\
                 {:#?}\n\
                 Internal state after another call to simplify():\n\
                 {:#?}\n\
                 Internal state after {} subsequent calls to \
                 complicate():\n\
                 {:#?}",
                before_simplified.current(),
                complicated.current(),
                num_simplifies,
                before_simplified,
                before_complicated,
                num_complications + 1,
                complicated
            );

            for iter in 1..16 {
                assert_same!(
                    prev_complicated.current(),
                    complicated.current(),
                    "complicate() returned false but changed the output \
                     value anyway.\n\
                     Old value:\n\
                     {:#?}\n\
                     New value:\n\
                     {:#?}\n\
                     Old internal state:\n\
                     {:#?}\n\
                     New internal state after {} calls to complicate()\
                     including the :\n\
                     {:#?}",
                    prev_complicated.current(),
                    complicated.current(),
                    prev_complicated,
                    iter,
                    complicated
                );

                assert!(
                    !complicated.complicate(),
                    "complicate() returned true after having returned \
                     false;\n\
                     Internal state before:\n{:#?}\n\
                     Internal state after calling complicate() {} times:\n\
                     {:#?}",
                    prev_complicated,
                    iter + 1,
                    complicated
                );
            }

            num_simplifies += 1;
            if num_simplifies > 65_536 {
                panic!(
                    "simplify() returned true over 65536 times in a row, \
                     aborting due to possible infinite loop. If this is not \
                     an infinite loop, it may be necessary to reconsider \
                     how shrinking is implemented or use a simpler test \
                     strategy. Internal state:\n{:#?}",
                    state
                );
            }
        }

        for iter in 0..16 {
            assert_same!(
                before_simplified.current(),
                state.current(),
                "simplify() returned false but changed the output \
                 value anyway.\n\
                 Old value:\n\
                 {:#?}\n\
                 New value:\n\
                 {:#?}\n\
                 Previous internal state:\n\
                 {:#?}\n\
                 New internal state after calling simplify() {} times:\n\
                 {:#?}",
                before_simplified.current(),
                state.current(),
                before_simplified,
                iter,
                state
            );

            if state.simplify() {
                panic!(
                    "simplify() returned true after having returned false. \
                     Previous internal state:\n\
                     {:#?}\n\
                     New internal state after calling simplify() {} times:\n\
                     {:#?}",
                    before_simplified,
                    iter + 1,
                    state
                );
            }
        }
    }
}
