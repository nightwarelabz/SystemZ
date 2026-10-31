# SystemZ loader build — freestanding C, no gnu-efi, no EDK2.
# GCC compiles to ELF objects; GNU ld's PE emulation for x86-64
# (i386pep) links the EFI application directly. Subsystem 10 = EFI
# application. Everything the loader links against is defined in efi.h.

CC      = gcc
LD      = ld
CFLAGS  = -ffreestanding -fno-stack-protector -fno-stack-check \
          -fno-ident -mno-red-zone -O2 -Wall -Wextra -std=c17 \
          -I. -I../boot -I../boot/generated
LDFLAGS = -nostdlib -mi386pep --subsystem 10 -e efi_main

BOOTX64.EFI: loader.o splash.o
	$(LD) $(LDFLAGS) -o $@ loader.o splash.o

loader.o: loader.c efi.h zboot.h ../boot/splash.h
	$(CC) $(CFLAGS) -c -o $@ loader.c

../boot/generated/splash_art.inc: ../boot/embed.py ../boot/assets/telepathy.rgb565
	python3 ../boot/embed.py $@

splash.o: ../boot/splash.c ../boot/splash.h ../boot/generated/splash_art.inc
	$(CC) $(CFLAGS) -mgeneral-regs-only -c -o $@ ../boot/splash.c

clean:
	rm -f loader.o splash.o BOOTX64.EFI

.PHONY: clean
