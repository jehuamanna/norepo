//! Shell-level reactive state shared across regions.
//!
//! The active activity-bar item drives which side-bar panel renders. We provide two
//! distinct wrapper types so `use_context` can resolve them by `TypeId` without collision.

use dioxus::prelude::*;

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct ActivityItemId(pub String);

#[derive(Clone, Copy)]
pub struct ActiveActivity(pub Signal<Option<ActivityItemId>>);

#[derive(Clone, Copy)]
pub struct LastActiveActivity(pub Signal<Option<ActivityItemId>>);
