# The Fedora 45 test images (nidhogg/ci/image.sh; README.md, "CI and releases"): a tarball's run-time packages
# (deps/fedora-45.runtime) and what the test scripts add, on the base pinned by digest.  The scripts check that their
# packages are installed (rpm -q) and install only when one is missing, so a stale list costs time, never a result;
# ci-test.sh checks that these lists name every package the scripts ask for.
#   gui (nidhogg-gui-fedora-45): gui.sh's packages for every lane (the mutter lanes' PipeWire set included; KWin's,
#       labwc's and the input methods' packages, three lanes' worth, stay with gui.sh); also the timing lane's image
#   pkg (nidhogg-pkg-fedora-45): pkg-test.sh's packages (the package suites)
FROM registry.fedoraproject.org/fedora@sha256:f5afe36c8ddb2aa5333597e900f53c5ed0267d9ebb27be765dc8ec2633aa1648 AS runtime
COPY fedora-45.runtime /usr/share/nidhogg-image/fedora-45.runtime
RUN dnf -y --setopt=install_weak_deps=False install $(sed 's/#.*//' /usr/share/nidhogg-image/fedora-45.runtime) && \
    dnf clean all

FROM runtime AS pkg
RUN dnf -y --setopt=install_weak_deps=False install \
      git make gcc cmake libtool clang-tools-extra python3-lsp-server gnutls-utils openssh-server openssh-clients \
      findutils procps-ng && \
    dnf clean all

FROM runtime AS gui
RUN dnf -y --setopt=install_weak_deps=False install \
      mutter sway grim pixman libxkbcommon libcap dbus-daemon mesa-dri-drivers mesa-libEGL mesa-libgbm \
      python3-gobject-base dejavu-sans-mono-fonts procps-ng util-linux \
      pipewire wireplumber gstreamer1-plugins-base pipewire-gstreamer pipewire-utils && \
    dnf clean all
