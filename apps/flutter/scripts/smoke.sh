#!/usr/bin/env bash
# Starts the built prototype ($1: the bundle), checks that it is still
# running after $SETTLE seconds, takes a screenshot of the screen and notes
# the app's resident memory and the bundle's sizes, then stops it. Results
# go to $2 (a directory: screenshot.png, results.txt). Linux runs it inside
# xvfb-run and a D-Bus session (ci.yml).
set -euo pipefail

bundle=$1
out=$(mkdir -p "$2" && cd "$2" && pwd)
settle=${SETTLE:-20}

case "$(uname -s)" in
  Linux) exe="$bundle/ppvpn_flutter" ;;
  Darwin) exe="$bundle/Contents/MacOS/ppvpn_flutter" ;;
  *) exe="$bundle/ppvpn_flutter.exe" ;;
esac

"$exe" >"$out/app.log" 2>&1 &
pid=$!
sleep "$settle"
if ! kill -0 "$pid" 2>/dev/null; then
  echo "::error::the prototype exited within ${settle}s"; cat "$out/app.log"; exit 1
fi

# Resident memory in KiB; on Windows Git Bash's pid is not the Windows one.
case "$(uname -s)" in
  Linux | Darwin) rss=$(ps -o rss= -p "$pid" | tr -d ' ') ;;
  *) rss=$(powershell -NoProfile -Command \
         "[int]((Get-Process ppvpn_flutter | Measure-Object WorkingSet64 -Sum).Sum / 1KB)") ;;
esac

case "$(uname -s)" in
  Linux) import -window root "$out/screenshot.png" ;;
  Darwin) screencapture -x "$out/screenshot.png" ;;
  *) powershell -NoProfile -Command "
       Add-Type -AssemblyName System.Windows.Forms, System.Drawing
       \$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
       \$img = New-Object System.Drawing.Bitmap \$b.Width, \$b.Height
       [System.Drawing.Graphics]::FromImage(\$img).CopyFromScreen(\$b.Location, [System.Drawing.Point]::Empty, \$b.Size)
       \$img.Save('$(cygpath -w "$out/screenshot.png")')" ;;
esac

case "$(uname -s)" in
  Linux | Darwin) kill "$pid"; wait "$pid" 2>/dev/null || true ;;
  *) taskkill //F //IM ppvpn_flutter.exe >/dev/null || true ;;
esac

# Unpacked size in KiB, and the size of a zip of the bundle (no installer
# yet: the closest to a download).
unpacked=$(du -sk "$bundle" | cut -f1)
(
  cd "$(dirname "$bundle")"
  if command -v zip >/dev/null; then zip -qry9 "$out/bundle.zip" "$(basename "$bundle")"
  else 7z a -tzip -mx=9 "$out/bundle.zip" "$(basename "$bundle")" >/dev/null; fi
)
zipped=$(( $(wc -c <"$out/bundle.zip") / 1024 ))
rm "$out/bundle.zip"
{
  echo "os=$(uname -s)"
  echo "rss_kib_after_${settle}s=$rss"
  echo "unpacked_kib=$unpacked"
  echo "zip_kib=$zipped"
} | tee "$out/results.txt"
