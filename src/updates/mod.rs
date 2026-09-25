//! Telling a changed value from a duplicate.
//!
//! Semantic dedup alone cannot: "the price is $5" and "the price is now $6"
//! embed almost identically. [`scan`] reads the business values a text
//! states and [`verdict`] compares two texts by them, deterministically.

pub mod lexicon;
pub mod plan;
pub mod recall;
pub mod scan;
pub mod store;
pub mod verdict;
