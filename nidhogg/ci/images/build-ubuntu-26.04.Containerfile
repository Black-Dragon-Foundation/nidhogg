# The Ubuntu 26.04 build image (nidhogg/ci/image.sh; README.md, "CI and releases"): deps/ubuntu-26.04.build and ccache
# on the base pinned by digest, installed as build.sh installs them (no recommends).  build.sh finds them installed and
# runs no apt-get.
FROM docker.io/library/ubuntu@sha256:6a06111c3b0a8600feadb270aec771dc239bf407b25625f93f44ab00e11f4d3c
COPY ubuntu-26.04.build /usr/share/nidhogg-image/ubuntu-26.04.build
RUN apt-get update && \
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      $(sed 's/#.*//' /usr/share/nidhogg-image/ubuntu-26.04.build) ccache && \
    rm -rf /var/lib/apt/lists/*
