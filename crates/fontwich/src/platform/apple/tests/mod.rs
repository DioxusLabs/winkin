//! Tests of the Core Text backend, of the Mac generics against Chrome's
//! settings, and of the sample letters against the installed fonts.

mod coverage;
mod generics;

use super::*;

#[test]
fn each_tradition_has_a_cascade() {
    for han in [
        None,
        Some(Han::Hans),
        Some(Han::Hant),
        Some(Han::Jpan),
        Some(Han::Kore),
    ] {
        assert!(
            !fetch_cascade(han).is_empty(),
            "an empty cascade for {han:?}"
        );
    }
}
