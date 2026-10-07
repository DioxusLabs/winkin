# Chrome locale settings fixtures

Verbatim copies of Chromium's `chrome/app/resources/locale_settings_*.grd`,
the default values of Chrome's per-script font settings on each desktop
platform. `src/fallback/generic/tests.rs` parses them and checks every cell
of `fallback::generic`'s tables against them. Copyright The Chromium Authors,
BSD-3-Clause, unmodified.

| file | Chromium ref | fetched |
| --- | --- | --- |
| `locale_settings_win.grd` | `refs/heads/main` | 2026-10-05 |
| `locale_settings_mac.grd` | `refs/heads/main` | 2026-10-05 |
| `locale_settings_linux.grd` | `refs/heads/main` | 2026-10-05 |

    https://chromium.googlesource.com/chromium/src/+/<ref>/chrome/app/resources/locale_settings_<platform>.grd?format=TEXT

A value here is a default only where `kFontDefaults` in
`chrome/browser/ui/prefs/prefs_tab_helper.cc` registers it for that
platform; the tests carry that list beside the files. The translations
(`platform_locale_settings/*.xtb`), which replace the Common values for a
browser in another UI language, are not copied: fontwich models the
English UI.
