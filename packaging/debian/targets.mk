PODMAN ?= podman
DISTRO ?= debian-13
DEB_DISTROS ?= debian-13 ubuntu-24.04
DEB_IMAGE = packaging/container/deb-image.sh

.PHONY: deb deb-build deb-ci deb-test deb-image-ref deb-image \
	deb-image-push deb-image-refs

# The package, built in the build image for $(DISTRO), into dist/.
deb: deb-image
	@$(PODMAN) run --rm -v "$(CURDIR)":/build:z -w /build \
		"$$($(DEB_IMAGE) --ref $(DISTRO))" make deb-build

# The package, built here, into dist/.
deb-build:
	@packaging/debian/build.sh

# Build, install and test the package, and check that a second build is the same.
deb-ci:
	@packaging/debian/ci.sh

deb-test:
	@packaging/debian/test.sh

deb-image-ref:
	@$(DEB_IMAGE) --ref $(DISTRO)

deb-image-refs:
	@for d in $(DEB_DISTROS); do \
		key=$$(printf '%s' "$$d" | tr '.-' '__'); \
		printf 'ref_%s=%s\n' "$$key" "$$($(DEB_IMAGE) --ref $$d)"; \
	done

deb-image:
	@$(DEB_IMAGE) --ensure $(DISTRO)

deb-image-push:
	@$(DEB_IMAGE) --push $(DISTRO)
