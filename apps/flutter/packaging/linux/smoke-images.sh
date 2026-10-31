# The containers package-smoke.sh runs in (ci.yml, release.yml), pinned by
# digest: the packages they install are fetched once per image and cached
# (package-smoke.sh), so a newer image would need them fetched again. Part
# of that cache's key.
SMOKE_IMAGE_DEB=ubuntu:24.04@sha256:fe81560990fae86dc4743202b2cb4d3df98d0bcf949173cecef15ce1127ef852
SMOKE_IMAGE_RPM=fedora:44@sha256:43b29f65a41eb9c35e1cd5323e3bdf3b655c2357a9f4f1ff2f9c2798e5045d80
