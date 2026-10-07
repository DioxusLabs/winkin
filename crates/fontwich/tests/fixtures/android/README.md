# Android `fonts.xml` fixtures

Verbatim copies of AOSP's `frameworks/base/data/fonts/fonts.xml`, used to test
the Android config parser on any host. Copyright The Android Open Source
Project, Apache-2.0, unmodified.

| file | AOSP ref | fetched |
| --- | --- | --- |
| `fonts-android-10.xml` | `refs/tags/android-10.0.0_r47` | 2026-09-11 |
| `fonts-android-13.xml` | `refs/tags/android-13.0.0_r83` | 2026-09-11 |
| `fonts-aosp-main.xml` | `refs/heads/main` | 2026-09-11 |

    https://android.googlesource.com/platform/frameworks/base/+/<ref>/data/fonts/fonts.xml?format=TEXT

## Why these three

Nine releases were surveyed — 10 through 17 and main, re-checked on
2026-09-21 — and between them they use exactly five elements (`familyset`,
`family`, `font`, `alias`, `axis`) and never change `familyset version="23"`.
These three cover every element and attribute the rest use:

- **10** is the format before anything was added: no `postScriptName` on
  `font`, no `ignore` on `family`.
- **13** has both, `postScriptName` having arrived in 12 and `ignore` in 13,
  and is otherwise identical in shape to 14 through 17.
- **main** carries the deprecation notice, which first appears in 15, and is
  what the format looks like now. It is byte-identical to 16 and 17.

The releases left out add nothing structural, so they would only cost
repository size. If a future release changes the shape, add it here rather
than editing one of these — the point of a fixture is that it is what
shipped. All three were re-fetched on 2026-09-21 and are byte-identical to
upstream at the refs below.

## What they are not

AOSP only. Every OEM skin is a genuine gap: Samsung, Xiaomi and others ship
their own, and Skia's own NDK font manager carries a workaround for Xiaomi
HyperOS naming `sans-serif` with no matching file. Nothing here proves how a
vendor build behaves, and a parser that passes on all three may still meet
something new on a real device.
