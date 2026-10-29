# SystemZ loader build — freestanding C, no gnu-efi, no EDK2.
# GCC compiles to ELF objects; GNU ld's PE emulation for x86-64
# (i386pep) links the EFI application directly. Subsystem 10 = EFI
# application. Everything the loader links against is defined in efi.h.

CC      = gcc
LD      = ld
CFLAGS  = -ffreestanding -fno-stack-protector -fno-stack-check \
          -fno-ident -mno-red-zone -O2 -Wall -Wextra -std=c17 -I.
LDFLAGS = -nostdlib -mi386pep --subsystem 10 -e efi_main

BOOTX64.EFI: loader.o
	$(LD) $(LDFLAGS) -o $@ loader.o

loader.o: loader.c efi.h zboot.h
	$(CC) $(CFLAGS) -c -o $@ loader.c

clean:
	rm -f loader.o BOOTX64.EFI

.PHONY: clean
