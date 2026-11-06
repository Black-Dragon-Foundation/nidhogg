# The Fedora 45 build image (nidhogg/ci/image.sh; README.md, "CI and releases"): deps/fedora-45.build and what the CI
# jobs add to it, on the base pinned by digest, so a job pulls one image instead of installing some 200 packages.
# build.sh, the crates' check.sh and workers-harness find their packages installed and skip the package manager.
#   norust (nidhogg-build-fedora-45-norust): every package but the Rust toolchain, for compile-noglitnir, which shows
#          the tree builds with no Rust installed
#   full   (nidhogg-build-fedora-45): plus Rust and the native crates' checks' packages
FROM registry.fedoraproject.org/fedora@sha256:f5afe36c8ddb2aa5333597e900f53c5ed0267d9ebb27be765dc8ec2633aa1648 AS norust
COPY fedora-45.build /usr/share/nidhogg-image/fedora-45.build
# ccache: NIDHOGG_BUILD_CCACHE; the sanitizer runtimes: workers-asan and workers-harness; gnutls-utils: the TLS rows'
# gnutls-serv (NIDHOGG_CHECK_PACKAGES); dbus-daemon: the private session bus of the settings tests (same list);
# diffutils: workers-harness's selftest
RUN dnf -y --setopt=install_weak_deps=False install \
      $(sed 's/#.*//' /usr/share/nidhogg-image/fedora-45.build | grep -vxE 'cargo|rust|rustc') \
      ccache libasan libubsan libtsan gnutls-utils dbus-daemon diffutils && \
    dnf clean all

FROM norust AS full
# the Rust packages of deps/fedora-45.build, then those of the recorder's, the shell's and the glue's check.sh
RUN dnf -y --setopt=install_weak_deps=False install \
      $(sed 's/#.*//' /usr/share/nidhogg-image/fedora-45.build | grep -xE 'cargo|rust|rustc') \
      clippy rustfmt cargo-deny binutils gawk openssl-devel cmake xkeyboard-config libX11-common pixman-devel && \
    dnf clean all
