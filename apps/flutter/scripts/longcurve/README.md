# The 24 h memory curve (Linux)

Acceptance 3 of #387, on the shared test machine (ppvpn-test). The run uses
Xvfb with Mesa's llvmpipe, so only the slope counts, not the absolute level.
It pushes the same synthetic traffic samples, one per second, as the 2 h
curve in CI. It sends nothing over the network.

## Steps

1. Take the Linux bundle from a CI run: the `flutter-bundle-linux` artifact,
   built by ci.yml's flutter job. Unpack it as `bundle/` next to these
   files. Nothing is compiled on the test machine.
2. Build the image: one hostq job, with apt going through the box's mirror.
   ```
   hostq run --owner <session> --group core --class build --cpus 2 --mem 2 --minutes 30 -- \
     docker build --build-arg MIRROR=http://mirrors.aliyun.com -t core-flutter-curve:1 .
   ```
3. Run the curve. It needs no network. The container is pinned to the
   cores hostq grants; `/out` keeps the samples across runs.
   ```
   hostq run --owner <session> --group core --class measurement --cpus 4 --mem 4 --minutes 1500 -- \
     sh -c 'mkdir -p "$HOME/flutter-curve-out" && docker run --rm --network none \
       --cpuset-cpus="$HOSTQ_CPUS" --memory 3g -v "$PWD/bundle:/app/bundle:ro" \
       -v "$HOME/flutter-curve-out:/out" core-flutter-curve:1'
   ```
   hostq streams every sample back to the submitting machine as it goes, so
   the samples are also copied off the box as they arrive.
4. After a preemption, run step 3 again. A new segment continues from the
   total already in `curve.csv`. If the box lost its disk, upload the last
   copy of `curve.csv` to `flutter-curve-out/` first. Report each segment's
   slope on its own, since a restarted app starts its memory from scratch.
