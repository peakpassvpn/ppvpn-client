FROM alpine:3.20@sha256:d9e853e87e55526f6b2917df91a2115c36dd7c696a35be12163d44e6e2a4b6bc
# APK_MIRROR (lab.sh: LAB_APK_MIRROR) replaces dl-cdn.alpinelinux.org on
# hosts that cannot reach it.
ARG APK_MIRROR=
RUN if [ -n "$APK_MIRROR" ]; then sed -i "s|https://dl-cdn.alpinelinux.org|$APK_MIRROR|" /etc/apk/repositories; fi && \
    apk add --no-cache curl python3 iptables ip6tables iproute2 bind-tools openssl socat jq ca-certificates procps tcpdump knot-utils coredns bc conntrack-tools netcat-openbsd
