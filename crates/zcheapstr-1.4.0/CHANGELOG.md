# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 1.4.0 - 2026-11-20

### ✨ Added
- Implement Borrow<str>, to look CheapStr keys up by &str.

## 1.3.0 - 2026-11-19

### ✨ Added
- Add CheapStr::as_borrowed, for a borrowed string that outlives self.

### 💥 Breaking
- (De)serialize CheapStr as a plain string in every format.

### 👷 CI
- Check the public API for semver violations in CI.
- Add a workflow to release with release-plz.

### 🐛 Fixed
- Copy a string that the deserializer can't lend instead of failing.

## 1.2.0 - 2026-10-29

### ✨ Added
- Make `CheapStr::as_str` a `const fn`.

### ♻️ Changed
- Show changelog emojis on section headings only.
- Support skipping the changelog via a commit trailer.

### ⚡️ Performance
- Copy borrowed data straight into an `Arc` when making it owned.

## 1.1.0 - 2026-08-10

### Added
- ✨ Add no_std support.
- 👷 Check no_std build in CI.

## 1.0.0 - 2026-08-09

- 🔖 First stable release. No changes over 0.1.0 — only a commitment to the
  current API. This matters especially because zvariant exposes `CheapStr` in
  its public API, and public dependencies of 1.0+ crates should themselves be
  1.0+.

## 0.1.0 - 2026-08-09

- 🎉 First release.

