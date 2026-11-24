//! 0.2.0 Studio: running any picture or video model the pinned engine supports. The
//! engine's own recipes are vendored in `catalog/sd-recipes.json` (design section 7.1).

pub mod index;
pub mod recipes;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
