#!/usr/bin/env python3
"""Boot the actual EFI + ZCore; record real framebuffer and serial evidence.

This is an emulator witness, never a physical-hardware or isolation claim.
Uses only Python's standard library and host QEMU/OVMF.
"""
from pathlib import Path
import json
import os
import shutil
import socket
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / 'tools/boot-splash/out/qemu'
OUT.mkdir(parents=True, exist_ok=True)
CODE = Path(os.environ.get('OVMF_CODE', '/usr/share/OVMF/OVMF_CODE_4M.fd'))
VARS = Path(os.environ.get('OVMF_VARS', '/usr/share/OVMF/OVMF_VARS_4M.fd'))
KERNEL = ROOT / 'zcore/target/x86_64-unknown-none/release/zcore'
for required in [CODE, VARS, KERNEL, ROOT / 'loader/BOOTX64.EFI']:
    if not required.is_file():
        raise SystemExit(f'missing boot witness input: {required}')


class Monitor:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.settimeout(10)
        self.sock.connect(str(path))
        self.io = self.sock.makefile('rwb', buffering=0)
        self.receive()
        self.command('qmp_capabilities')

    def receive(self):
        while True:
            raw = self.io.readline()
            if not raw:
                raise RuntimeError('QMP closed')
            value = json.loads(raw)
            if 'event' not in value:
                return value

    def command(self, execute, arguments=None):
        request = {'execute': execute}
        if arguments is not None:
            request['arguments'] = arguments
        self.io.write(json.dumps(request).encode() + b'\n')
        result = self.receive()
        if 'error' in result:
            raise RuntimeError(result)
        return result

    def close(self):
        self.io.close()
        self.sock.close()


def check_framebuffer(path, failure):
    raw = path.read_bytes()
    header, pixels = raw.split(b'\n255\n', 1)
    tokens = header.split()
    assert tokens[0] == b'P6', 'QMP screenshot is not PPM'
    width, height = map(int, tokens[1:3])
    assert len(pixels) == width * height * 3
    scale = min(width // 640, height // 360)
    assert scale >= 1
    left, top = (width - 640 * scale) // 2, (height - 360 * scale) // 2
    # A known glyph pixel from the actual post-handoff diagnostic screen,
    # or the red live failure progress bar. This cannot pass on OVMF's logo.
    x, y = (220, 290) if failure else (35, 32)
    offset = ((top + y * scale) * width + left + x * scale) * 3
    expected = bytes.fromhex('d87969' if failure else 'cbd3c5')
    assert pixels[offset:offset + 3] == expected, (
        f'{path.name}: framebuffer did not contain SystemZ state at {x},{y}: '
        f'{pixels[offset:offset+3].hex()} != {expected.hex()}')


def witness(case):
    with tempfile.TemporaryDirectory(prefix='sz-boot-') as temp:
        work = Path(temp)
        esp = work / 'esp'
        (esp / 'EFI/BOOT').mkdir(parents=True)
        (esp / 'EFI/SystemZ').mkdir(parents=True)
        shutil.copyfile(ROOT / 'loader/BOOTX64.EFI', esp / 'EFI/BOOT/BOOTX64.EFI')
        if case != 'missing-kernel':
            target = esp / 'EFI/SystemZ/zcore.elf'
            if case == 'invalid-kernel':
                target.write_bytes(b'invalid ELF witness' * 8)
            else:
                shutil.copyfile(KERNEL, target)
        shutil.copyfile(VARS, work / 'vars.fd')
        log = OUT / f'{case}.log'
        qmp = work / 'qmp.sock'
        command = [
            'qemu-system-x86_64', '-machine', 'q35', '-accel', 'tcg',
            '-m', '256', '-cpu', 'max',
            '-drive', f'if=pflash,format=raw,unit=0,readonly=on,file={CODE}',
            '-drive', f'if=pflash,format=raw,unit=1,file={work / "vars.fd"}',
            '-drive', f'format=raw,file=fat:rw:{esp}',
            '-serial', f'file:{log}', '-display', 'none',
            '-qmp', f'unix:{qmp},server=on,wait=off', '-no-reboot',
            '-vga', 'none' if case == 'no-graphics' else 'std',
        ]
        stderr = (OUT / f'{case}.stderr').open('wb')
        process = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=stderr)
        monitor = None
        sent_escape = False
        failure = case in ('missing-kernel', 'invalid-kernel')
        try:
            deadline = time.monotonic() + 45
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError(f'{case}: QEMU exited {process.returncode}')
                if monitor is None and qmp.exists():
                    monitor = Monitor(qmp)
                text = log.read_text(errors='replace') if log.exists() else ''
                if case == 'escape-details' and not sent_escape and monitor and 'GOP framebuffer accepted' in text:
                    monitor.command('send-key', {'keys': [{'type': 'qcode', 'data': 'esc'}]})
                    sent_escape = True
                finished = 'loader: FAILED' in text if failure else 'splash: diagnostics handoff complete' in text
                if finished:
                    if failure:
                        assert 'SystemZ ZCore' not in text, 'bad kernel was entered'
                    else:
                        assert 'SystemZ ZCore' in text
                        assert 'zboot: memory map:' in text
                        assert 'verdict:' in text
                        if 'svm (amd):   true' in text:
                            assert 'npt: self-check PASS' in text, 'upstream NPT census regressed'
                            assert 'GUEST BORN' in text or 'guest birth NOT CLAIMED' in text
                        assert 'vmx: readiness' in text, 'upstream VMX readiness report lost'
                    if case == 'normal':
                        assert 'zboot: framebuffer accepted' in text
                    if case == 'no-graphics':
                        assert 'text / serial fallback active' in text
                        assert 'graphical splash disabled' in text
                    if case == 'escape-details':
                        assert 'firmware boot details' in text, 'Escape did not reach loader'
                        assert 'graphical splash disabled' in text
                    if case in ('normal', 'missing-kernel', 'invalid-kernel'):
                        assert monitor
                        screen = OUT / f'{case}.ppm'
                        monitor.command('screendump', {'filename': str(screen)})
                        check_framebuffer(screen, failure)
                    print(f'PASS: actual UEFI -> ZCore boot witness: {case}', flush=True)
                    return
                time.sleep(.025)
            raise RuntimeError(f'{case}: timed out; see {log}')
        finally:
            if monitor:
                monitor.close()
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            stderr.close()


for name in ('normal', 'missing-kernel', 'invalid-kernel', 'no-graphics', 'escape-details'):
    witness(name)
print('All boot witnesses passed. Emulator evidence only; physical hardware unproven.')
