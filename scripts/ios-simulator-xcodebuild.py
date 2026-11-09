#!/usr/bin/env python3
"""Tauri's --no-sign omits simulator Keychain entitlements. Ask Xcode to
produce a normal ad-hoc simulator identity instead; never use a developer
account or provisioning portal. This adapter refuses hardware/archive exports.
"""
import os
from pathlib import Path
import sys

args = sys.argv[1:]
if '-sdk' not in args or args[args.index('-sdk') + 1] != 'iphonesimulator':
    raise SystemExit('MPAC local signing adapter accepts only the iOS simulator SDK')
root = Path(os.environ.get('MPAC_MOBILE_ROOT', Path(__file__).resolve().parents[1] if Path(__file__).parent.name == 'scripts' else Path(__file__).resolve().parents[2]))
if not (root / 'mobile/src-tauri/Entitlements.simulator.plist').is_file():
    raise SystemExit('Could not locate the simulator app identity')
filtered = [arg for arg in args if not arg.startswith(('CODE_SIGNING_', 'CODE_SIGN_IDENTITY=', 'CODE_SIGN_ENTITLEMENTS=')) and arg != '-allowProvisioningUpdates']
filtered += ['CODE_SIGNING_ALLOWED=YES', 'CODE_SIGNING_REQUIRED=YES', 'CODE_SIGN_IDENTITY=-', 'CODE_SIGN_STYLE=Manual', 'DEVELOPMENT_TEAM=',
             'CODE_SIGN_ENTITLEMENTS=' + str(root / 'mobile/src-tauri/Entitlements.simulator.plist')]
os.execv('/Applications/Xcode.app/Contents/Developer/usr/bin/xcodebuild', ['xcodebuild', *filtered])
