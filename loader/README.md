# loader/

Our own UEFI boot loader (SPEC.md §§4, 10, 19): verifies under Secure
Boot with operator-owned keys, measures ZCore + the generation seal into
the TPM, loads ZCore at its linked base, and enters `_start` with the
boot manifest (current generation, trial generation if any).

**State: not started.** M1's first-boot exit proof (SPEC.md §21) is a
cold power-on through this loader on real x86_64 hardware; until then,
nothing in this repo boots, and nothing here claims otherwise.
