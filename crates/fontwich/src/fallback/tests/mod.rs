//! Tests of the key, the character classifier and the generic families.
//!
//! `generic` also reads Chrome's settings for each platform's generic tests.

mod classify;
mod generic;
mod key;

pub(crate) use generic::{
    BUCKETS, COLUMNS, GENERICS, SETTINGS, bucket, drawn, grd_families, message_id,
    registered_on_desktop, registered_on_mac_and_windows,
};
