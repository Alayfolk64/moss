# Build Options
export ARCH := riscv64
export LOG := warn
export DWARF := n
export MODE := release
export MEMTRACK := n
export SHELL_INIT ?= n
export RUSTUP_TOOLCHAIN ?= nightly-2025-05-20
RUSTUP ?= rustup
RUST_LLD ?= rust-lld
RUST_OBJCOPY ?= rust-objcopy
LOONGARCH_CC ?=
export RUSTFLAGS ?= -Zcrate-attr=feature(unsigned_signed_diff)

# QEMU Options
export BLK := y
export NET := n
export VSOCK := n
export MEM := 1G
export ICOUNT := n
export BUS := mmio

# Generated Options
export A := $(PWD)
export NO_AXSTD := y
export AX_LIB := axfeat
export APP_FEATURES := qemu
CARGO_CONFIG_TEMPLATE := $(A)/cargo-config.toml

ifeq ($(MEMTRACK), y)
	APP_FEATURES += starry-api/memtrack
endif

ifeq ($(SHELL_INIT), y)
	APP_FEATURES += shell-init
endif

default: build

ensure-cargo-config:
	@mkdir -p $(A)/.cargo
	@cp -f $(CARGO_CONFIG_TEMPLATE) $(A)/.cargo/config.toml

toolchain: ensure-cargo-config
	@$(RUSTUP) default $(RUSTUP_TOOLCHAIN)

KERNEL_NAME := $(notdir $(A))
SDCARD_RV_IMG := $(A)/sdcard-rv.img
SDCARD_RV_SRC := $(A)/.sdcard-rv.src.img
SDCARD_LA_IMG := $(A)/sdcard-la.img
SDCARD_LA_SRC := $(A)/.sdcard-la.src.img
RUNTIME_DISK_RV_IMG := $(A)/disk.img
RUNTIME_DISK_LA_IMG := $(A)/disk-la.img
RUNTIME_DISK_ROOT := $(A)/.runtime-disk-root
RV_LIBGCC_SRC := /opt/riscv64-linux-musl-cross/riscv64-linux-musl/lib/libgcc_s.so.1
LA_LIBGCC_SRC := /opt/gcc-13.2.0-loongarch64-linux-gnu/loongarch64-linux-gnu/lib64/libgcc_s.so.1
RV_MUSL_LIB_DIR := /opt/riscv64-linux-musl-cross/riscv64-linux-musl/lib
LA_MUSL_LIB_DIR := /opt/loongarch64-linux-musl-cross/loongarch64-linux-musl/lib
RV_MUSL_LIBC_SRC := $(RV_MUSL_LIB_DIR)/libc.so
LA_MUSL_LIBC_SRC := $(LA_MUSL_LIB_DIR)/libc.so

ROOTFS_URL = https://github.com/Starry-OS/rootfs/releases/download/20260214
ROOTFS_IMG = rootfs-$(ARCH).img

rootfs:
	@if [ ! -f $(ROOTFS_IMG) ]; then \
		echo "Image not found, downloading..."; \
		curl -f -L $(ROOTFS_URL)/$(ROOTFS_IMG).xz -O; \
		xz -d $(ROOTFS_IMG).xz; \
	fi
	@cp $(ROOTFS_IMG) make/disk.img

img:
	@echo -e "\033[33mWARN: The 'img' target is deprecated. Please use 'rootfs' instead.\033[0m"
	@$(MAKE) --no-print-directory rootfs

defconfig justrun: toolchain
	@$(MAKE) -C make $@

makeclean:
	@$(MAKE) -C make makeclean
	@rm -f $(A)/kernel-rv $(A)/kernel-la $(A)/kernel-rv.bin $(A)/kernel-la.bin \
		$(A)/kernel-rv.dbg.elf $(A)/kernel-la.dbg.elf \
		$(A)/kernel-rv.bin.o $(A)/kernel-la.bin.o $(A)/kernel-la.bin.S

prepare-sdcard-rv:
	@src="$(SDCARD_RV_SRC)"; \
		img="$(SDCARD_RV_IMG)"; \
		if [ ! -f "$$src" ]; then \
			if [ -e "$$img" ]; then \
				cp -Lf "$$img" "$$src"; \
			elif [ -f "$(A)/rootfs-riscv64.img" ]; then \
				cp -f "$(A)/rootfs-riscv64.img" "$$src"; \
			elif [ -f "$(A)/make/disk.img" ]; then \
				cp -f "$(A)/make/disk.img" "$$src"; \
			else \
				$(MAKE) --no-print-directory ARCH=riscv64 rootfs; \
				cp -f "$(A)/rootfs-riscv64.img" "$$src"; \
			fi; \
		fi; \
		rm -f "$$img"; \
		cp -f "$$src" "$$img"

prepare-sdcard-la:
	@src="$(SDCARD_LA_SRC)"; \
		img="$(SDCARD_LA_IMG)"; \
		if [ ! -f "$$src" ]; then \
			if [ -e "$$img" ]; then \
				cp -Lf "$$img" "$$src"; \
			elif [ -f "$(A)/rootfs-loongarch64.img" ]; then \
				cp -f "$(A)/rootfs-loongarch64.img" "$$src"; \
			elif [ -f "$(A)/make/disk.img" ]; then \
				cp -f "$(A)/make/disk.img" "$$src"; \
			else \
				$(MAKE) --no-print-directory ARCH=loongarch64 rootfs; \
				cp -f "$(A)/rootfs-loongarch64.img" "$$src"; \
			fi; \
		fi; \
		rm -f "$$img"; \
		cp -f "$$src" "$$img"

clean: makeclean
	@:

build run debug disasm: toolchain defconfig
	@$(MAKE) -C make $@

all: kernel-rv kernel-la

kernel-rv: toolchain
	$(MAKE) ARCH=riscv64 build
	cp -f $(A)/$(KERNEL_NAME)_riscv64-qemu-virt.elf $(A)/kernel-rv.dbg.elf
	cp -f $(A)/$(KERNEL_NAME)_riscv64-qemu-virt.bin $(A)/kernel-rv.bin
	$(RUST_OBJCOPY) -I binary -O elf64-littleriscv -B riscv --rename-section .data=.text,alloc,load,readonly,code $(A)/kernel-rv.bin $(A)/kernel-rv.bin.o
	$(RUST_LLD) -flavor gnu -m elf64lriscv -T $(A)/make/kernel-rv-wrap.ld -o $(A)/kernel-rv $(A)/kernel-rv.bin.o
	rm -f $(A)/kernel-rv.bin.o

kernel-la: toolchain
	$(MAKE) ARCH=loongarch64 BUS=pci build
	cp -f $(A)/$(KERNEL_NAME)_loongarch64-qemu-virt.elf $(A)/kernel-la.dbg.elf
	cp -f $(A)/$(KERNEL_NAME)_loongarch64-qemu-virt.bin $(A)/kernel-la.bin
	@LOONGARCH_CC_BIN="$(LOONGARCH_CC)"; \
	if [ -z "$$LOONGARCH_CC_BIN" ]; then \
		if command -v loongarch64-linux-musl-gcc >/dev/null 2>&1; then \
			LOONGARCH_CC_BIN=loongarch64-linux-musl-gcc; \
		elif command -v loongarch64-linux-gnu-gcc >/dev/null 2>&1; then \
			LOONGARCH_CC_BIN=loongarch64-linux-gnu-gcc; \
		else \
			echo "missing loongarch64 cross gcc"; \
			exit 1; \
		fi; \
	fi; \
	printf '%s\n' \
		'.section .text,"ax",@progbits' \
		'.global _binary_kernel_la_bin_start' \
		'.global _binary_kernel_la_bin_end' \
		'.balign 16' \
		'_binary_kernel_la_bin_start:' \
		'.incbin "kernel-la.bin"' \
		'_binary_kernel_la_bin_end:' > $(A)/kernel-la.bin.S; \
	"$$LOONGARCH_CC_BIN" -c -o $(A)/kernel-la.bin.o $(A)/kernel-la.bin.S
	$(RUST_LLD) -flavor gnu -m elf64loongarch -T $(A)/make/kernel-la-wrap.ld -o $(A)/kernel-la $(A)/kernel-la.bin.o
	rm -f $(A)/kernel-la.bin.o $(A)/kernel-la.bin.S

prepare-runtime-disk-root:
		@echo "[runtime-disk] prepare begin: root=$(RUNTIME_DISK_ROOT)"
		@rm -rf "$(RUNTIME_DISK_ROOT)"
		@mkdir -p "$(RUNTIME_DISK_ROOT)/runtime/riscv64" "$(RUNTIME_DISK_ROOT)/runtime/loongarch64"
		@echo "starry-runtime-disk" > "$(RUNTIME_DISK_ROOT)/.starry-runtime-disk"
		@echo "v1" > "$(RUNTIME_DISK_ROOT)/.starry-runtime-layout"
		@if [ -f "$(RV_LIBGCC_SRC)" ]; then cp -f "$(RV_LIBGCC_SRC)" "$(RUNTIME_DISK_ROOT)/runtime/riscv64/libgcc_s.so.1"; echo "[runtime-disk] OK: $(RV_LIBGCC_SRC) -> runtime/riscv64/libgcc_s.so.1"; else echo "[runtime-disk] WARN: missing $(RV_LIBGCC_SRC)"; fi
		@if [ -f "$(LA_LIBGCC_SRC)" ]; then cp -f "$(LA_LIBGCC_SRC)" "$(RUNTIME_DISK_ROOT)/runtime/loongarch64/libgcc_s.so.1"; echo "[runtime-disk] OK: $(LA_LIBGCC_SRC) -> runtime/loongarch64/libgcc_s.so.1"; else echo "[runtime-disk] WARN: missing $(LA_LIBGCC_SRC)"; fi
		@if [ -f "$(RV_MUSL_LIBC_SRC)" ]; then \
			cp -f "$(RV_MUSL_LIBC_SRC)" "$(RUNTIME_DISK_ROOT)/runtime/riscv64/ld-musl-riscv64.so.1"; \
			echo "[runtime-disk] OK: $(RV_MUSL_LIBC_SRC) -> runtime/riscv64/ld-musl-riscv64.so.1"; \
		else \
			echo "[runtime-disk] WARN: missing $(RV_MUSL_LIBC_SRC)"; \
		fi
		@if [ -f "$(LA_MUSL_LIBC_SRC)" ]; then \
			cp -f "$(LA_MUSL_LIBC_SRC)" "$(RUNTIME_DISK_ROOT)/runtime/loongarch64/ld-musl-loongarch64.so.1"; \
			echo "[runtime-disk] OK: $(LA_MUSL_LIBC_SRC) -> runtime/loongarch64/ld-musl-loongarch64.so.1"; \
		else \
			echo "[runtime-disk] WARN: missing $(LA_MUSL_LIBC_SRC)"; \
		fi
		@echo "[runtime-disk] prepare end"

disk.img: prepare-runtime-disk-root
	@if ! command -v mke2fs >/dev/null 2>&1; then echo "[disk.img] ERROR: mke2fs not found"; exit 1; fi
	@rm -f "$(RUNTIME_DISK_RV_IMG)"
	@mke2fs -q -t ext4 -d "$(RUNTIME_DISK_ROOT)" -L STARRYRT -F "$(RUNTIME_DISK_RV_IMG)" 16M
	@rm -rf "$(RUNTIME_DISK_ROOT)"
	@echo "[disk.img] generated $(RUNTIME_DISK_RV_IMG)"

disk-la.img: prepare-runtime-disk-root
	@if ! command -v mke2fs >/dev/null 2>&1; then echo "[disk-la.img] ERROR: mke2fs not found"; exit 1; fi
	@rm -f "$(RUNTIME_DISK_LA_IMG)"
	@mke2fs -q -t ext4 -d "$(RUNTIME_DISK_ROOT)" -L STARRYRT -F "$(RUNTIME_DISK_LA_IMG)" 16M
	@rm -rf "$(RUNTIME_DISK_ROOT)"
	@echo "[disk-la.img] generated $(RUNTIME_DISK_LA_IMG)"

contest-run-rv: defconfig
	@test -f "$(A)/sdcard-rv.img" || (echo "missing image: $(A)/sdcard-rv.img" && exit 1)
	@$(MAKE) -C make run ARCH=riscv64 BLK=y NET=n BUS=mmio DISK_IMG=$(A)/sdcard-rv.img

repair-sdcard-rv:
	@test -f "$(A)/sdcard-rv.img" || (echo "missing image: $(A)/sdcard-rv.img" && exit 1)
	@docker run --rm --platform linux/amd64 \
		-v "$(A):/work" \
		-w /work \
		docker.io/zhouzhouyi/os-contest:20260104 \
		bash -lc 'e2fsck -fy /work/sdcard-rv.img >/tmp/e2fsck-rv.log 2>&1; rc=$$?; cat /tmp/e2fsck-rv.log; [ $$rc -le 2 ]'

ci-test:
	./scripts/ci-test.py $(ARCH)

# Aliases
rv:
	$(MAKE) ARCH=riscv64 run

rv-shell:
	$(MAKE) ARCH=riscv64 SHELL_INIT=y run

la:
	$(MAKE) ARCH=loongarch64 BUS=pci run

la-shell:
	$(MAKE) ARCH=loongarch64 BUS=pci SHELL_INIT=y kernel-la
	@img="$(A)/sdcard-la.img"; \
	tmp_img="$(A)/tmp-la-shell.img"; \
	if [ ! -f "$$img" ]; then \
		$(MAKE) --no-print-directory prepare-sdcard-la; \
	fi; \
	rm -f "$$tmp_img"; \
	cp -f "$$img" "$$tmp_img"; \
	qemu-system-loongarch64 -kernel $(A)/kernel-la -m 1G -nographic -smp 1 \
		-drive file=$$tmp_img,if=none,format=raw,snapshot=on,id=x0 \
		-device virtio-blk-pci,drive=x0 \
		-no-reboot \
		-device virtio-net-pci,netdev=net0 \
		-netdev user,id=net0 \
		-serial mon:stdio \
		-rtc base=utc

vf2:
	$(MAKE) ARCH=riscv64 APP_FEATURES=vf2 MYPLAT=axplat-riscv64-visionfive2 BUS=mmio build

docker:
	docker run -it --rm --privileged \
	  --platform linux/amd64 \
	  -w /work \
	  -v $(CURDIR):/work \
	  docker.io/zhouzhouyi/os-contest:20260104 \
	  /bin/bash

.PHONY: all build run justrun debug disasm clean makeclean kernel-rv kernel-la disk.img disk-la.img prepare-runtime-disk-root contest-run-rv repair-sdcard-rv toolchain ensure-cargo-config prepare-sdcard-rv prepare-sdcard-la rv-shell la-shell docker
