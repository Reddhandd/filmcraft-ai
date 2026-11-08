# First independent public alpha

Package: 0.13.0-ai-alpha.3, installer revision 1. Prepared for publication 2026-11-08.

The installer is the previously qualified, unchanged arm64 package, not a new application build.
Its editor/helper were built from application commit `c7d49b332bd67261929cc10c06c9f1e4abb13cc0`;
installer metadata preservation was fixed at `44280666c6e83458b31af094265a249e367140f8`.
These are local development provenance identifiers, not promises that those original commits
are publicly retrievable. Local development history remains retained by the owner.
The public source change is consolidated over upstream commit
`7428957ae0ece1139ae3af7ea76e97cacdb69570` to exclude owner-specific test identifiers from its history.
The public source tree additionally removes upstream trademark artwork, simplifies the already
mark-free alpha wordmark path, substitutes synthetic test IDs, and adds publication documentation.
Rebuilding from this tree produces a new binary; byte-for-byte reproduction is not claimed.
The separate uncommitted welcome-screen work is not included in this release.

## Qualification and boundaries

Prior complete workspace gates recorded 4,475 passed tests and 52 skipped tests. Native private-profile
checks covered save/reopen, export, library import/retrieval/OCR, assistant teaching update and restore,
autosave, requested page images and fresh assistant-context retrieval. Attended testing restored voice,
working context and editing after reconnect. These results are scoped to the developer's Apple Silicon
Mac and tested provider/client versions; they do not qualify all Macs, files or provider updates.

The final r1 disk image was manually mounted and its app inventory matched the signed staging bundle.
The read-only payload signature check passed. The original qualification was ad-hoc signing,
not notarization or Gatekeeper approval on another machine. Public delivery includes checksums and
sanitized qualification metadata, without the private test books or source project data.

The source publication gate is rerun before committing this consolidated release tree.
No app installation, owner-profile migration, microphone session or GUI launch is part of publication.
