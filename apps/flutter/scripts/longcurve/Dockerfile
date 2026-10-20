# The long memory curve's container (#387): what the Linux bundle needs to
# run under Xvfb with Mesa's llvmpipe, nothing else. Debian 13, pinned by
# digest; the test machine pulls it through its registry mirror and gets
# packages from its Debian mirror (MIRROR).
FROM debian:trixie-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f
ARG MIRROR=http://deb.debian.org
RUN sed -i "s#http://deb.debian.org#${MIRROR}#g" /etc/apt/sources.list.d/debian.sources \
 && apt-get update \
 && apt-get install -y --no-install-recommends xvfb xauth procps dbus dbus-x11 libgtk-3-0t64 \
      libgl1 libegl1 libgles2 libgl1-mesa-dri mesa-vulkan-drivers libstdc++6 fonts-dejavu-core ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY run.sh /run.sh
ENTRYPOINT ["/bin/bash", "/run.sh"]
