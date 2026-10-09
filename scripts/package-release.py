#!/usr/bin/env python3
"""Package an already-built native release, with documentation and licenses."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parent.parent

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--platform', choices=['macos-arm64', 'macos-x86_64', 'linux-x86_64'], required=True)
    args = parser.parse_args()
    version = re.search(r'^version = "([0-9]+\.[0-9]+\.[0-9]+)"', (ROOT/'crates/ferrender/Cargo.toml').read_text(), re.M)[1]
    tag = os.environ.get('RELEASE_TAG', 'v' + version)
    if tag != 'v' + version:
        raise SystemExit('Release tag does not match Cargo version')
    commit = subprocess.check_output(['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], text=True).strip()
    mac = args.platform.startswith('macos')
    binary = ROOT/('dist/Ferrender.app/Contents/MacOS/ferrender' if mac else 'target/release/ferrender')
    info = subprocess.check_output([str(binary), '--version'], text=True)
    target = {'macos-arm64': 'aarch64-apple-darwin', 'macos-x86_64': 'x86_64-apple-darwin', 'linux-x86_64': 'x86_64-unknown-linux-gnu'}[args.platform]
    # The executable records at compile time which OpenCascade directory cadrum
    # linked; a build that bypassed scripts/prepare-occt.py says "unverified".
    pins = json.loads((ROOT/'scripts/occt-pins.json').read_text())
    occt = f"OpenCascade: verified {pins['tag']} sha256:{pins['assets'][target]['sha256']}"
    for expected in [f'Ferrender {version}', f'Commit: {commit}', 'Source: Clean checkout', 'Build: release', f'Platform: {target}', occt]:
        if expected not in info:
            raise SystemExit(f'Build metadata mismatch: expected {expected!r}; got {info!r}')
    # CI and the macOS bundler prepare this exact native dependency before
    # compiling. Recheck its archive when packaging instead of silently using
    # an arbitrary pre-existing target/occt-* directory for release notices.
    occt_root = Path(subprocess.check_output(['python3', str(ROOT/'scripts/prepare-occt.py'), '--target', target], text=True).strip())
    selected_occt = os.environ.get('OCCT_ROOT')
    if selected_occt and Path(selected_occt).resolve() != occt_root.resolve():
        raise SystemExit('OCCT_ROOT differs from the verified release dependency; rebuild with scripts/prepare-occt.py')
    name = f'Ferrender-{version}-{args.platform}'
    stage = ROOT/'dist'/name
    if stage.exists():
        shutil.rmtree(stage)
    stage.mkdir(parents=True)
    if mac:
        subprocess.run(['codesign', '--verify', '--deep', '--strict', str(binary.parents[2])], check=True)
        shutil.copytree(binary.parents[2], stage/'Ferrender.app', symlinks=True)
    else:
        shutil.copy2(binary, stage/'ferrender')
        shutil.copy2(ROOT/'assets/icon.png', stage/'Ferrender.png')
        dependencies = subprocess.check_output(['ldd', str(binary)], text=True)
        if 'not found' in dependencies:
            raise SystemExit('A runtime dependency is missing: ' + dependencies)
        (stage/'LINUX-DEPENDENCIES.txt').write_text(dependencies)
    for doc in ['README.md', 'BISHOP_TUTORIAL.md', 'RELEASING.md']:
        shutil.copy2(ROOT/doc, stage/doc)
    shutil.copytree(ROOT/'docs', stage/'docs')
    (stage/'BUILD-INFO.txt').write_text(info)
    licenses = stage/'licenses'
    licenses.mkdir()
    for filename in ['OFL.txt', 'PROVENANCE.txt']:
        shutil.copy2(ROOT/'crates/fr-core/assets/fonts'/filename, licenses/('NotoSans-'+filename))
    # Cadrum's pinned OCCT prebuilt archive ships the LGPL and additional exception.
    for filename in ['LICENSE_LGPL_21.txt', 'OCCT_LGPL_EXCEPTION.txt']:
        notice = occt_root/'share/doc/opencascade'/filename
        if not notice.is_file():
            raise SystemExit(f'Missing bundled OpenCascade notice: {filename}')
        shutil.copy2(notice, licenses/filename)
    registry = Path(os.environ.get('CARGO_HOME', str(Path.home()/'.cargo')))/'registry/src'
    cadrum = sorted(registry.glob('*/cadrum-0.8.20/LICENSE'))
    if not cadrum:
        raise SystemExit('Missing cadrum license')
    shutil.copy2(cadrum[0], licenses/'cadrum-MIT.txt')
    (licenses/'SOURCE.txt').write_text('Ferrender source and build instructions: https://github.com/base698/ferrender/tree/'+tag+'\nOpenCascade source: https://github.com/Open-Cascade-SAS/OCCT/tree/V8_0_1\ncadrum source: https://crates.io/crates/cadrum/0.8.20\n')
    install = 'macOS: Move Ferrender.app to Applications (or your Desktop). This build is ad-hoc signed, not notarized. If macOS blocks the downloaded app, use System Settings > Privacy & Security > Open Anyway after reviewing the source. Choose the download matching Apple Silicon or Intel.\n' if mac else 'Linux x86-64: Extract the entire archive and run ./ferrender from the extracted folder. Built and tested on Ubuntu 24.04 (glibc 2.39); older distributions may not run it. A desktop session with X11 or Wayland and a working OpenGL/Vulkan driver is required. On Ubuntu, runtime packages include libx11-6 libxkbcommon0 libwayland-client0 libegl1 libgl1 libvulkan1 mesa-vulkan-drivers libstdc++6 xdg-desktop-portal.\n'
    (stage/'INSTALL.txt').write_text(install+'\nOpen Help > About Ferrender to verify version and commit. Documentation: README.md and BISHOP_TUTORIAL.md.\n')
    # Verify the packaged CLI can create a real solid; no GUI or existing document is touched.
    request = {'jsonrpc': '2.0', 'id': 1, 'method': 'tools/call', 'params': {'name': 'execute_ferrender_commands', 'arguments': {'commands': [
        {'op': 'new', 'units': 'mm'}, {'op': 'create_sketch', 'plane': 'XY'},
        {'op': 'add_geometry', 'items': [{'type': 'rect', 'from': [0,0], 'to': [10,20]}]},
        {'op': 'extrude', 'distance': 3}, {'op': 'get_scene_info'}]}}}
    packaged = stage/('Ferrender.app/Contents/MacOS/ferrender' if mac else 'ferrender')
    result = subprocess.run([str(packaged), 'mcp', '--headless'], input=json.dumps(request)+'\n', text=True, capture_output=True, check=True, timeout=60)
    result = json.loads(result.stdout)['result']
    if result.get('isError'):
        raise SystemExit(result)
    scene = json.loads(result['content'][0]['text'])[-1]
    if len(scene['bodies']) != 1 or scene['bodies'][0]['size'] != [10.0, 20.0, 3.0] or scene['bodies'][0]['open_edges'] != 0:
        raise SystemExit('Packaged geometry smoke test failed')
    archive = ROOT/'dist'/(name+('.zip' if mac else '.tar.gz'))
    archive.unlink(missing_ok=True)
    if mac:
        subprocess.run(['ditto', '-c', '-k', '--sequesterRsrc', '--keepParent', str(stage), str(archive)], check=True)
    else:
        with tarfile.open(archive, 'w:gz') as tar:
            tar.add(stage, arcname=name)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_name(archive.name+'.sha256').write_text(digest+'  '+archive.name+'\n')
    print(archive)
    print(info)

if __name__ == '__main__':
    main()
