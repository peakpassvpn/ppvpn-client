#!/usr/bin/env bash
# Build one isolated checkout at a time. Never installs or uploads artifacts.
# Both comparison sides must use the same SDK/NDK/JDK/signing/build number.
# Usage: build-size-artifact.sh CHECKOUT OUTPUT apk|aab|apk-split|all [--preflight]
set -euo pipefail
checkout=${1:?isolated clean checkout required}
output=${2:?output directory required}
kind=${3:?apk, aab, apk-split or all required}
case "$kind" in apk|aab|apk-split|all) ;; *) echo 'unsupported Android measurement kind' >&2; exit 2 ;; esac
if [ "$(uname -s)" != Linux ]; then
  echo 'The existing Android app recipe requires a Linux SDK-equipped build host.' >&2
  exit 1
fi
: "${ANDROID_NDK_HOME:?real Android NDK required}"
sdk=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}
[ -n "$sdk" ] && [ -d "$sdk/platforms" ] && [ -d "$sdk/build-tools" ] ||
  { echo 'Android SDK platforms/build-tools are missing' >&2; exit 1; }
for command in java flutter cargo python3 keytool jarsigner; do
  command -v "$command" >/dev/null || { echo "missing build tool: $command" >&2; exit 1; }
done
for target in aarch64-linux-android x86_64-linux-android; do
  [ -x "$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/${target}26-clang" ] ||
    { echo "NDK lacks API-26 compiler for $target" >&2; exit 1; }
done
[ -z "$(git -C "$checkout" status --porcelain)" ] ||
  { echo 'Use a clean isolated checkout; generated or diagnostic changes are not a source baseline.' >&2; exit 1; }
[ "${4:-}" != --preflight ] || { echo 'Android build preflight passed; nothing compiled.'; exit 0; }
# The build may create generated files in this checkout. Never point this at
# a read-only reference checkout or an installed application's source tree.
mkdir -p "$output"
output=$(cd "$output" && pwd)
python3 - "$output" <<'PY'
from pathlib import Path
import sys
if any(Path(sys.argv[1]).iterdir()):
    raise SystemExit("Use a new empty output directory; retained artifacts are never overwritten.")
PY
python3 - "$checkout" <<'PY'
import shutil,sys
if shutil.disk_usage(sys.argv[1]).free < 12 * 1024**3:
    raise SystemExit('Less than 12 GiB free before Android build; no cache or checkout cleanup is performed.')
PY
commit=$(git -C "$checkout" rev-parse HEAD)
format=apk
[ "$kind" != aab ] || format=aab
export PPVPN_BUILD_NUMBER=${PPVPN_BUILD_NUMBER:-343}
: "${PPVPN_API_BASE_DEFAULT:?explicit equal measurement backend required on both builds}"
FLUTTER_TARGET=android ANDROID_FORMAT="$format" "$checkout/apps/flutter/scripts/ci-build.sh"
version=$(python3 - "$checkout/Cargo.toml" <<'PY'
import sys,tomllib
print(tomllib.load(open(sys.argv[1], 'rb'))['workspace']['package']['version'])
PY
)
if [ "$kind" = apk ] || [ "$kind" = all ]; then
  cp "$checkout/apps/flutter/build/app/outputs/flutter-apk/app-release.apk" "$output/"
fi
if [ "$kind" = apk-split ] || [ "$kind" = all ]; then
  (cd "$checkout/apps/flutter" && flutter build apk --release --split-per-abi \
    --target-platform android-arm64,android-x64 --build-name "$version" --build-number "$PPVPN_BUILD_NUMBER")
  for abi in arm64-v8a x86_64; do
    cp "$checkout/apps/flutter/build/app/outputs/flutter-apk/app-$abi-release.apk" "$output/"
  done
fi
if [ "$kind" = all ]; then
  (cd "$checkout/apps/flutter" && flutter build appbundle --release \
    --target-platform android-arm64,android-x64 --build-name "$version" --build-number "$PPVPN_BUILD_NUMBER")
fi
if [ "$kind" = aab ] || [ "$kind" = all ]; then
  cp "$checkout/apps/flutter/build/app/outputs/bundle/release/app-release.aab" "$output/"
fi
# Keep exact provenance without printing backend URLs, signing secrets or host paths.
python3 - "$checkout" "$output" "$commit" "$kind" <<'PY'
import hashlib,json,os,pathlib,re,subprocess,sys
checkout,output,commit,kind=sys.argv[1:]
ndk=pathlib.Path(os.environ['ANDROID_NDK_HOME'])/'source.properties'
revision=next((line.partition('=')[2].strip() for line in ndk.read_text().splitlines()
               if line.startswith('Pkg.Revision')), None)
sdk=pathlib.Path(os.environ.get('ANDROID_HOME') or os.environ['ANDROID_SDK_ROOT'])
java = str(pathlib.Path(os.environ['JAVA_HOME']) / 'bin/java') if os.environ.get('JAVA_HOME') else 'java'
jdk=subprocess.run([java,'-version'],capture_output=True,text=True,check=True)
certificates={}
for artifact in sorted(pathlib.Path(output).glob('*.apk')):
    signers=list((sdk/'build-tools').glob('*/apksigner'))
    if not signers:
        raise SystemExit('apksigner missing; artifact signing was not verified')
    signer=max(signers,key=lambda p: tuple(int(n) for n in re.findall(r'\d+',p.parent.name)))
    result=subprocess.run([str(signer),'verify','--print-certs',str(artifact)],
                          capture_output=True,text=True,check=True)
    hashes=re.findall(r'certificate SHA-256 digest: ([0-9a-fA-F]+)',result.stdout)
    if not hashes:
        raise SystemExit('APK signature certificate digest missing')
    certificates[artifact.name]=sorted(set(h.lower() for h in hashes))
for artifact in sorted(pathlib.Path(output).glob('*.aab')):
    result=subprocess.run(['jarsigner','-J-Duser.language=en','-verify',str(artifact)],
                          capture_output=True,text=True,check=True)
    if 'jar verified.' not in result.stdout:
        raise SystemExit('AAB JAR signature was not verified')
    result=subprocess.run(['keytool','-J-Duser.language=en','-printcert','-jarfile',str(artifact)],
                          capture_output=True,text=True,check=True)
    hashes=re.findall(r'SHA256:\s*([0-9a-fA-F:]+)',result.stdout)
    if not hashes:
        raise SystemExit('AAB signature certificate digest missing')
    certificates[artifact.name]=sorted(set(h.replace(':','').lower() for h in hashes))
result={'source_commit':commit,'kind':kind,'abis':['arm64-v8a','x86_64'],
        'sdk_platforms_available':sorted(p.name for p in (sdk/'platforms').iterdir() if p.is_dir()),
        'sdk_build_tools_available':sorted(p.name for p in (sdk/'build-tools').iterdir() if p.is_dir()),
        'jdk_version':(jdk.stderr or jdk.stdout).splitlines()[0],
        'build_number':os.environ['PPVPN_BUILD_NUMBER'],'ndk_revision':revision,
        'cargo_lock_sha256':hashlib.sha256((pathlib.Path(checkout)/'Cargo.lock').read_bytes()).hexdigest(),
        'backend_origin_sha256':hashlib.sha256(os.environ['PPVPN_API_BASE_DEFAULT'].encode()).hexdigest(),
        'rust':subprocess.check_output(['rustc','--version'],text=True).strip(),
        'flutter':json.loads(subprocess.check_output(['flutter','--version','--machine'],text=True)),
        'signing':'provided keystore' if os.environ.get('ANDROID_KEYSTORE_FILE') else 'measurement debug key',
        'verified_signing_certificate_sha256':certificates,
        'generated_changes':subprocess.check_output(['git','-C',checkout,'diff','--name-only'],text=True).splitlines(),
        'note':'No install, push message, publication or Play device download measurement performed.'}
(pathlib.Path(output)/'build-provenance.json').write_text(json.dumps(result,indent=2)+'\n')
PY
