# The Fedora 45 check image (nidhogg/ci/image.sh; README.md, "CI and releases"): deps/fedora-45.check, the packages of
# the shell's, the glue's and Glitnir's check.sh --container, on the base pinned by digest.  Built locally once per tag
# (image.sh local check-fedora-45): a check run then starts cargo without a package manager.
FROM registry.fedoraproject.org/fedora@sha256:f5afe36c8ddb2aa5333597e900f53c5ed0267d9ebb27be765dc8ec2633aa1648
COPY fedora-45.check /usr/share/nidhogg-image/fedora-45.check
RUN dnf -y --setopt=install_weak_deps=False install $(sed 's/#.*//' /usr/share/nidhogg-image/fedora-45.check) && \
    dnf clean all
