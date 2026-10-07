//! Tests of the Windows tables against Chrome and Unicode, and of the
//! sample letters against DirectWrite's fonts.

// DirectWrite's fonts, so on Windows only. The rest is data, on any host.
#[cfg(windows)]
mod coverage;
mod generics;
mod oracle;
