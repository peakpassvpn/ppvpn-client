#!/usr/bin/env python3
"""Verify actual exported App/profile APNs equality without printing secrets."""
import plistlib
import argparse
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('app', type=Path)
parser.add_argument('--testflight', action='store_true')
args = parser.parse_args()
app = args.app
info = plistlib.loads((app / 'Info.plist').read_bytes())
if info.get('PPVPNApplePushEnabled') is not True:
    print('Apple push rollout disabled; no delivery acceptance claimed')
    raise SystemExit(0)

def reject(message):
    raise SystemExit('Apple push packaging check: ' + message)

entitlement = subprocess.run(['codesign', '-d', '--entitlements', ':-', str(app)], capture_output=True)
if entitlement.returncode:
    reject('enabled package must have an actual code signature')
if subprocess.run(['codesign', '--verify', '--strict', str(app)], capture_output=True).returncode:
    reject('enabled package signature did not verify')
try:
    signed = plistlib.loads(entitlement.stdout)
    profile = plistlib.loads(subprocess.check_output(['security', 'cms', '-D', '-i', str(app / 'embedded.mobileprovision')], stderr=subprocess.DEVNULL))['Entitlements']
except (ValueError, FileNotFoundError, KeyError, subprocess.CalledProcessError):
    reject('signed entitlements and embedded profile must both be readable')
environment = signed.get('aps-environment')
if environment not in ('development', 'production'):
    reject('signed APNs environment is absent or unknown')
if args.testflight and environment != 'production':
    reject('TestFlight requires production APNs')
if profile.get('aps-environment') != environment or info.get('PPVPNAPNsEntitlementEnvironment') != environment:
    reject('signed, embedded-profile and declared APNs environments differ')
identifier = signed.get('application-identifier', '')
topic = info.get('CFBundleIdentifier')
if not topic or not identifier.endswith('.' + topic) or profile.get('application-identifier') != identifier:
    reject('exact application/topic identity differs from the embedded profile')
print('Signed APNs environment, topic and embedded profile agree')
