#!/usr/bin/env python3
"""Project-local overlays after Tauri init; no service/config/credential changes."""
from pathlib import Path
import plistlib
import shutil
import os
import json
import re
import sys

root = Path(__file__).resolve().parents[1]
native = root / 'mobile/src-tauri'
icons = native / 'icons'
icons.mkdir(exist_ok=True)
shutil.copyfile(root / 'desktop/src-tauri/icons/icon.png', icons / 'icon.png')
cli = root / 'desktop/node_modules/@tauri-apps/cli/tauri.js'
for directory in [native, native / 'gen/apple']:
    if directory.exists():
        relative = os.path.relpath(cli, directory)
        task_root = os.path.relpath(root, directory)
        (directory / 'tauri').write_text(
            'const path = require("node:path");\n'
            'const root = path.resolve(__dirname, ' + json.dumps(task_root) + ');\n'
            'process.env.CARGO_HOME = path.join(root, ".tools/cargo");\n'
            'process.env.RUSTUP_HOME = path.join(root, ".tools/rustup");\n'
            'process.env.PATH = path.join(root, ".tools/cargo/bin") + path.delimiter + process.env.PATH;\n'
            'require(path.resolve(__dirname, ' + json.dumps(relative) + '));\n')
android = native / 'gen/android/app/build.gradle.kts'
if android.exists():
    for duplicate in (native / 'gen/android/app/src/main/jniLibs').glob('**/libmpac_mobile_lib *.so'):
        if duplicate.is_symlink() and duplicate.resolve().is_file() and duplicate.resolve().name == 'libmpac_mobile_lib.so':
            duplicate.unlink()
    # Force a compact fresh archive rather than retaining obsolete ZIP entries
    # from repeated local incremental APK packaging.
    apk = native / 'gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk'
    if apk.is_file() and 'android' in sys.argv[1:]:
        apk.unlink()
    shutil.copyfile(root / 'mobile/native/android/MainActivity.kt', native / 'gen/android/app/src/main/java/com/mpac/mobile/MainActivity.kt')
    android_icons = icons / 'android'
    if android_icons.exists():
        shutil.copytree(android_icons, native / 'gen/android/app/src/main/res', dirs_exist_ok=True)
    data = android.read_text()
    # Use the locally installed SDK. No license/SDK auto-upgrade is necessary.
    data = data.replace('compileSdk = 37', 'compileSdk = 36').replace('targetSdk = 37', 'targetSdk = 36')
    # Keep symbols in Cargo outputs for developers, but do not ship hundreds
    # of megabytes of native debug information in the local APK.
    data = re.sub(r'^\s*jniLibs\.keepDebugSymbols\.add\([^\n]*\)\n', '', data, flags=re.MULTILINE)
    android.write_text(data)
    manifest = native / 'gen/android/app/src/main/AndroidManifest.xml'
    text = manifest.read_text()
    # Credentials must not be backed up/restored onto another phone.
    if 'android:allowBackup=' not in text:
        text = text.replace('<application', '<application\n        android:allowBackup="false"', 1)
    # Wry requests RECORD_AUDIO through the Android permission dialog.
    for declaration in (
        '<uses-permission android:name="android.permission.RECORD_AUDIO" />',
        '<uses-permission android:name="android.permission.MODIFY_AUDIO_SETTINGS" />',
        '<uses-feature android:name="android.hardware.microphone" android:required="false" />',
    ):
        if declaration not in text:
            text = text.replace('    <application', f'    {declaration}\n    <application', 1)
    manifest.write_text(text)
# Xcode resolves signing inputs before the Rust/plugin pre-build phase. Write
# associated domains up front; otherwise the first archive silently lacks them.
entitlements = native / 'gen/apple/mpac-mobile_iOS/mpac-mobile_iOS.entitlements'
if entitlements.exists():
    with entitlements.open('rb') as source: values = plistlib.load(source)
    config = json.loads((native / 'tauri.conf.json').read_text())
    domains = sorted({
        'applinks:' + item['host']
        for item in config.get('plugins', {}).get('deep-link', {}).get('mobile', [])
        if item.get('appLink') and item.get('host') and 'https' in item.get('scheme', [])
    })
    if domains: values['com.apple.developer.associated-domains'] = domains
    else: values.pop('com.apple.developer.associated-domains', None)
    with entitlements.open('wb') as target: plistlib.dump(values, target)
apple = native / 'gen/apple/mpac-mobile_iOS/Info.plist'
if apple.exists():
    with apple.open('rb') as source: data = plistlib.load(source)
    with (native / 'Info.ios.plist').open('rb') as source: data.update(plistlib.load(source))
    with apple.open('wb') as target: plistlib.dump(data, target)
    ios_icons = icons / 'ios'
    if ios_icons.exists():
        shutil.copytree(ios_icons, native / 'gen/apple/Assets.xcassets/AppIcon.appiconset', dirs_exist_ok=True)
print('Mobile native overlays prepared')
