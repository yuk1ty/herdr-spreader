//! Verus type recognitions for the config data model.
//!
//! The `Ex*` structs tell Verus what the crate's config types look like.
//! They exist only for verification and are erased when building with the
//! normal toolchain. Types that a specification never inspects are opaque
//! (`#[verifier::external_body]`); the rest expose their fields so specs can
//! read them.
#![allow(dead_code)]

use crate::config::{Pane, SplitDirection, Tab, WaitFor};
use std::path::PathBuf;

use verus_builtin_macros::verus;
// Used only by the `verus!` block below; erased builds see no vstd items.
#[allow(unused_imports)]
use vstd::prelude::*;

verus! {

#[verifier::external_type_specification]
#[verifier::external_body]
pub struct ExPathBuf(PathBuf);

#[verifier::external_type_specification]
#[verifier::external_body]
pub struct ExSplitDirection(SplitDirection);

#[verifier::external_type_specification]
#[verifier::external_body]
pub struct ExWaitFor(WaitFor);

#[verifier::external_type_specification]
pub struct ExPane(Pane);

#[verifier::external_type_specification]
pub struct ExTab(Tab);

} // verus!
