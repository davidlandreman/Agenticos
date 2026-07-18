# CA certificates

`cacert.pem` is the pinned Mozilla CA extraction published by the curl
project. AgenticOS stages the committed snapshot and imports it as
`/etc/ssl/cert.pem` only when the kernel has a valid wall clock. Links2 uses
that system trust store and does not ship a separate embedded root set.

Run `make verify` to check the committed artifact. To update it, change the
date and SHA-256 in the Makefile, run `make refresh`, review the extraction
date and certificate changes, and commit the new bundle.

The extracted certificate data is distributed under the Mozilla Public
License 2.0; see <https://www.mozilla.org/MPL/2.0/>. The extraction is
documented at <https://curl.se/docs/caextract.html>.
