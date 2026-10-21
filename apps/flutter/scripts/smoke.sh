#!/usr/bin/env bash
# Starts the built prototype ($1: the bundle), checks that it is still
# running after $SETTLE seconds, takes a screenshot of the screen and notes
# the app's resident memory and the bundle's sizes, then stops it; then
# screenshots of the signed-in pages on sample data. Results go to $2 (a
# directory: screenshot.png, overview-light.png, overview-dark.png,
# nodes-light.png, logs-light.png, messages-light.png, results.txt). Linux
# runs it inside
# nodes-light.png, settings-light.png, results.txt). Linux runs it inside
# xvfb-run and a D-Bus session (ci.yml).
set -euo pipefail

bundle=$1
out=$(mkdir -p "$2" && cd "$2" && pwd)
settle=${SETTLE:-20}

case "$(uname -s)" in
  Linux) exe="$bundle/ppvpn" ;;
  Darwin) exe="$bundle/Contents/MacOS/PPVPN" ;;
  *) exe="$bundle/ppvpn.exe" ;;
esac

"$exe" >"$out/app.log" 2>&1 &
pid=$!
sleep "$settle"
if ! kill -0 "$pid" 2>/dev/null; then
  echo "::error::the prototype exited within ${settle}s"; cat "$out/app.log"
  # macOS: the crash report (the thread that crashed and its frames).
  # ReportCrash writes it a few seconds after the crash.
  if [ "$(uname -s)" = Darwin ]; then
    sleep 15
    report=$(ls -t ~/Library/Logs/DiagnosticReports/*.ips /Library/Logs/DiagnosticReports/*.ips \
      2>/dev/null | head -1 || true)
    if [ -n "$report" ]; then echo "$report"; head -c 30000 "$report"; else echo "no crash report"; fi
  fi
  exit 1
fi

# Resident memory in KiB; on Windows Git Bash's pid is not the Windows one.
case "$(uname -s)" in
  Linux | Darwin) rss=$(ps -o rss= -p "$pid" | tr -d ' ') ;;
  *) rss=$(powershell -NoProfile -Command \
         "[int]((Get-Process ppvpn | Measure-Object WorkingSet64 -Sum).Sum / 1KB)") ;;
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
  *) taskkill //F //IM ppvpn.exe >/dev/null || true ;;
esac

# Screenshots of the signed-in pages on the design's sample data
# (lib/sample.dart; no client runs), light and dark.
shot() {
  local name=$1; shift
  env "$@" "$exe" >"$out/$name.log" 2>&1 &
  local p=$!
  sleep 8
  case "$(uname -s)" in
    Linux) import -window root "$out/$name.png" ;;
    Darwin) screencapture -x "$out/$name.png" ;;
    *) powershell -NoProfile -Command "
         Add-Type -AssemblyName System.Windows.Forms, System.Drawing
         \$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
         \$img = New-Object System.Drawing.Bitmap \$b.Width, \$b.Height
         [System.Drawing.Graphics]::FromImage(\$img).CopyFromScreen(\$b.Location, [System.Drawing.Point]::Empty, \$b.Size)
         \$img.Save('$(cygpath -w "$out/$name.png")')" ;;
  esac
  case "$(uname -s)" in
    Linux | Darwin) kill "$p"; wait "$p" 2>/dev/null || true ;;
    *) taskkill //F //IM ppvpn.exe >/dev/null || true ;;
  esac
}
shot overview-light PPVPN_FLUTTER_SAMPLE=overview PPVPN_FLUTTER_THEME=light
shot overview-dark PPVPN_FLUTTER_SAMPLE=overview PPVPN_FLUTTER_THEME=dark
shot nodes-light PPVPN_FLUTTER_SAMPLE=nodes PPVPN_FLUTTER_THEME=light
shot logs-light PPVPN_FLUTTER_SAMPLE=logs PPVPN_FLUTTER_THEME=light
shot messages-light PPVPN_FLUTTER_SAMPLE=messages PPVPN_FLUTTER_THEME=light
shot settings-light PPVPN_FLUTTER_SAMPLE=settings PPVPN_FLUTTER_THEME=light

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
