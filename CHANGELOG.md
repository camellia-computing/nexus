# Changelog

All notable changes to Camellia Nexus will be recorded here from the first
commercial release onward.

## [1.0.2] - 2026-08-05

### Fixed

- harden Nexus client runtime boundaries (#18)


## [1.0.1] - 2026-07-31

### Fixed

- isolate expected private trust probe failure (#13)

- reconcile completed publication lifecycle (#14)


## [1.0.0] - 2026-07-31

### Added

- establish Camellia Nexus production baseline

- publish verified signing trust metadata (#5)

- freeze and verify client publication (#12)


### Build

- update ajv to 8.18.0


### Changed

- bound glib advisory exception

- harden canonical version parsing


### Fixed

- constrain privilege broker target input (#11)


## [Unreleased]

- Establish the clean 0.1.0 pre-release product baseline.
- Adopt the Camellia proprietary authorization terms.
- Use the canonical `camellia.nexus.license` authorization scope.
- Accept only the current local data schema; no development-build migration path is supported.
