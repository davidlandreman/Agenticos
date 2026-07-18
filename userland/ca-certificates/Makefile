SHELL := /bin/sh

CURL ?= curl
CA_BUNDLE_DATE := 2026-07-16
CA_BUNDLE_URL := https://curl.se/ca/cacert-$(CA_BUNDLE_DATE).pem
CA_BUNDLE_SHA256 := 3ff344e30b9b1ed2971044eabb438a08f2e2245ddb5f8ab1a3ad8b63ab4eaf91

.PHONY: all verify refresh clean

all: cacert.pem

cacert.pem:
	$(CURL) -fL --retry 3 -o $@.tmp $(CA_BUNDLE_URL)
	printf '%s  %s\n' '$(CA_BUNDLE_SHA256)' '$@.tmp' | shasum -a 256 -c -
	mv -f $@.tmp $@

verify: cacert.pem
	printf '%s  %s\n' '$(CA_BUNDLE_SHA256)' '$<' | shasum -a 256 -c -

# Updating the snapshot is deliberate: change the date and digest together,
# run make refresh, review the Mozilla extraction date, then commit cacert.pem.
refresh:
	rm -f cacert.pem
	$(MAKE) cacert.pem

clean:
	rm -f cacert.pem.tmp
