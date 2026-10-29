# SystemZ live boot splash

The original blurry alien and recursive artwork is a **text-free texture**.
The wordmark, moving tunnel texture, progress segments, status, failures and
transition into diagnostics are drawn by `splash.c` in the actual boot path.
The previous generated picture with a loading bar is not used.

The same freestanding C renderer is compiled into the UEFI loader and ZCore.
It needs no graphics library, runtime allocator, floating point, firmware
calls after handoff, or external code. The checked-in 384x128 little-endian
RGB565 texture deliberately retains the original low resolution. It came
from the text-free alien telepathy artwork supplied in this task; it is
embedded at build time by the standard-library-only `embed.py`.

## What advances the screen

Each segment corresponds to a completed operation, not a timer percentage:

| Completed | Evidence |
|---|---|
| 1 | A supported GOP framebuffer has been obtained and validated |
| 2 | The ZCore file is open on a boot volume |
| 3 | The entire ZCore file has been read |
| 4 | ELF segments have been validated and placed by the existing loader |
| 5 | The boot manifest and final UEFI memory map are prepared |
| 6 | Execution has reached ZCore after ExitBootServices |
| 7 | ZCore's CPUID probe has completed |
| 8 | The boot memory map has been reported |
| 9 | The implemented guest probe has returned, with its actual outcome |

There is a 12-frame, approximately 300 ms reveal before file loading, and
additional animation frames while reading 64 KiB chunks and changing stages.
The reveal does not advance progress. Define `SZ_BOOT_REVEAL_FRAMES=0` to
remove it. A blocked read can stop animation; there is no scheduler yet.

Once the implemented checks end, the splash is replaced by **ZCore early
boot diagnostics** showing the real virtualization/guest outcome. There is
no invented desktop or success claim: ZAdmin and the operator shell are not
implemented. Unsupported hardware, failed guests and unproven guest entry
are identified. This PR implements presentation and the existing boot-chain
handoff, not the rest of the operating system.

## Firmware and ABI

GOP RGBX, BGRX and disjoint eight-bit channel masks are supported. The
renderer respects framebuffer size and scanline stride, clips writes, and
centres an integer-scaled 640x360 layout without stretching. Unsupported
formats, BLT-only modes, modes smaller than 640x360, or absent GOP retain
firmware text plus COM1 serial output. Escape before ExitBootServices
switches to firmware boot details and disables graphics for this boot.

`loader/zboot.h` is the canonical ABI. Manifest v2 preserves the complete
72-byte v1 prefix and appends a 48-byte framebuffer value plus flags/frame.
No firmware protocol pointers cross the boundary. ZCore still accepts v1.
Framebuffer MMIO and manifest pages must remain mapped and reserved when a
future memory manager takes over. No UEFI calls occur after the final map
capture except ExitBootServices and its permitted map/exit retry; fatal
paths after the first exit attempt use direct framebuffer and serial only.

## Build and proof

```sh
make -C loader
(cd zcore && cargo build --release --locked)
bash tools/boot-splash/check.sh
python3 tools/boot-splash/qemu.py
```

Build inputs: x86_64 GCC/binutils, Python 3 and the Rust bare-metal target.
ZCore's build script uses `SYSTEMZ_CC` / `SYSTEMZ_AR` if supplied. The
guarded host checks use ASan/UBSan and cover five resolutions, all supported
pixel layouts, scanline padding, out-of-bounds guards, animation changes,
stage/error rendering, diagnostic transition and malformed framebuffers.
On a ptrace-constrained host, run the host checker with
`ASAN_OPTIONS=detect_leaks=0`; this retains address/undefined checks.

The QEMU witness boots the **actual EFI application and Rust ZCore** under
OVMF. It checks normal handoff, missing/malformed kernel failures, no-GOP
fallback and Escape. QMP screenshots are checked for pixels belonging to
SystemZ's live states, so a firmware logo cannot count as a passing splash.
Artifacts include the real EFI/ZCore binaries, serial logs and framebuffer
captures. `renderer-preview.mp4` is explicitly a host renderer demonstration;
it does not claim a real machine boot. Physical-hardware proof remains owed.
