# =============================================================================
#  KopitOS — atajos de make (Linux, macOS, WSL, MSYS2)
#  Todo delega en `cargo kopit`, que funciona igual en Windows sin make.
#
#    make run                    UEFI en QEMU (perfil desktop)
#    make run BOOT=pvh           arranque directo
#    make run ARCH=riscv64       RISC-V (o ARCH=aarch64)
#    make iso PROFILE=server RELEASE=1
#    make test-all               pruebas en las tres arquitecturas
# =============================================================================

PROFILE ?= desktop
ARCH    ?= x86_64
BOOT    ?= uefi
RELEASE ?=
CMDLINE ?=

KOPIT := cargo kopit
FLAGS := --profile $(PROFILE) --arch $(ARCH)
ifneq ($(RELEASE),)
FLAGS += --release
endif
ifneq ($(CMDLINE),)
FLAGS += --cmdline "$(CMDLINE)"
endif

.PHONY: all build image iso run run-headless test test-all grub-iso profiles doctor fmt clippy clean help

all: image

build:
	$(KOPIT) build $(FLAGS)

image:
	$(KOPIT) image $(FLAGS)

iso:
	$(KOPIT) iso $(FLAGS)

run:
	$(KOPIT) run $(FLAGS) --boot $(BOOT)

run-headless:
	$(KOPIT) run $(FLAGS) --boot $(BOOT) --headless

test:
	$(KOPIT) test $(FLAGS)

test-all:
	$(KOPIT) test --profile $(PROFILE) --arch x86_64
	$(KOPIT) test --profile $(PROFILE) --arch aarch64
	$(KOPIT) test --profile $(PROFILE) --arch riscv64

grub-iso:
	$(KOPIT) grub-iso $(FLAGS)

profiles:
	$(KOPIT) profiles

doctor:
	$(KOPIT) doctor --arch $(ARCH)

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace --exclude kopit-kernel --exclude kopitboot
	cargo clippy -p kopit-kernel --target x86_64-unknown-none
	cargo clippy -p kopitboot --target x86_64-unknown-uefi

clean:
	$(KOPIT) clean
	cargo clean

help:
	$(KOPIT) help
