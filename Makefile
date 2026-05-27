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
GDB ?= gdb-multiarch
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

debug-rv:
	$(MAKE) ARCH=riscv64 SHELL_INIT=y DWARF=y kernel-rv
	@img="$(A)/sdcard-rv.img"; \
	tmp_img="$(A)/tmp-rv-shell.img"; \
	if [ ! -f "$$img" ]; then \
		$(MAKE) --no-print-directory prepare-sdcard-rv; \
	fi; \
	rm -f "$$tmp_img"; \
	cp -f "$$img" "$$tmp_img"
	@printf "    \033[36mDebug image\033[0m: kernel-rv.dbg.elf\n"
	@printf "    \033[36mGDB\033[0m: $(GDB) kernel-rv.dbg.elf -ex 'target remote localhost:1234'\n"
	qemu-system-riscv64 -machine virt -kernel kernel-rv -m $(MEM) -nographic -smp 1 -bios default \
		-drive file=tmp-rv-shell.img,if=none,format=raw,id=x0 \
		-device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
		-no-reboot -device virtio-net-device,netdev=net \
		-netdev user,id=net -rtc base=utc -S -s 2>&1 | tee output_rv.txt

debug-la:
	$(MAKE) ARCH=loongarch64 BUS=pci SHELL_INIT=y DWARF=y kernel-la
	@img="$(A)/sdcard-la.img"; \
	tmp_img="$(A)/tmp-la-shell.img"; \
	if [ ! -f "$$img" ]; then \
		$(MAKE) --no-print-directory prepare-sdcard-la; \
	fi; \
	rm -f "$$tmp_img"; \
	cp -f "$$img" "$$tmp_img"
	@printf "    \033[36mDebug image\033[0m: kernel-la.dbg.elf\n"
	@printf "    \033[36mGDB\033[0m: $(GDB) kernel-la.dbg.elf -ex 'target remote localhost:1234'\n"
	qemu-system-loongarch64 -kernel kernel-la -m $(MEM) -nographic -smp 1 \
		-drive file=tmp-la-shell.img,if=none,format=raw,id=x0 \
		-device virtio-blk-pci,drive=x0 -no-reboot \
		-device virtio-net-pci,netdev=net0 -netdev user,id=net0 \
		-rtc base=utc -S -s 2>&1 | tee output_la.txt

debug-rv-timerfd:
	$(MAKE) ARCH=riscv64 DWARF=y APP_FEATURES="qemu timerfd-debug-init" kernel-rv
	@img="$(A)/sdcard-rv.img"; \
	tmp_img="$(A)/tmp-rv-shell.img"; \
	if [ ! -f "$$img" ]; then \
		$(MAKE) --no-print-directory prepare-sdcard-rv; \
	fi; \
	rm -f "$$tmp_img"; \
	cp -f "$$img" "$$tmp_img"
	@printf "    \033[36mDebug image\033[0m: kernel-rv.dbg.elf\n"
	@printf "    \033[36mGDB\033[0m: $(GDB) kernel-rv.dbg.elf -ex 'target remote localhost:1234'\n"
	qemu-system-riscv64 -machine virt -kernel kernel-rv -m $(MEM) -nographic -smp 1 -bios default \
		-drive file=tmp-rv-shell.img,if=none,format=raw,id=x0 \
		-device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
		-no-reboot -device virtio-net-device,netdev=net \
		-netdev user,id=net -rtc base=utc -S -s 2>&1 | tee output_rv.txt

debug-la-timerfd:
	$(MAKE) ARCH=loongarch64 BUS=pci DWARF=y APP_FEATURES="qemu timerfd-debug-init" kernel-la
	@img="$(A)/sdcard-la.img"; \
	tmp_img="$(A)/tmp-la-shell.img"; \
	if [ ! -f "$$img" ]; then \
		$(MAKE) --no-print-directory prepare-sdcard-la; \
	fi; \
	rm -f "$$tmp_img"; \
	cp -f "$$img" "$$tmp_img"
	@printf "    \033[36mDebug image\033[0m: kernel-la.dbg.elf\n"
	@printf "    \033[36mGDB\033[0m: $(GDB) kernel-la.dbg.elf -ex 'target remote localhost:1234'\n"
	qemu-system-loongarch64 -kernel kernel-la -m $(MEM) -nographic -smp 1 \
		-drive file=tmp-la-shell.img,if=none,format=raw,id=x0 \
		-device virtio-blk-pci,drive=x0 -no-reboot \
		-device virtio-net-pci,netdev=net0 -netdev user,id=net0 \
		-rtc base=utc -S -s 2>&1 | tee output_la.txt

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
	$(MAKE) ARCH=riscv64 SHELL_INIT=y kernel-rv
	@img="$(A)/sdcard-rv.img"; \
	tmp_img="$(A)/tmp-rv-shell.img"; \
	if [ ! -f "$$img" ]; then \
		$(MAKE) --no-print-directory prepare-sdcard-rv; \
	fi; \
	rm -f "$$tmp_img"; \
	cp -f "$$img" "$$tmp_img"; \
	qemu-system-riscv64 -machine virt -kernel kernel-rv -m 1G -nographic -smp 1 -bios default -drive file=sdcard-rv.img,if=none,format=raw,id=x0 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -no-reboot -device virtio-net-device,netdev=net -netdev user,id=net -rtc base=utc 2>&1 | tee output_rv.txt


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
	qemu-system-loongarch64 -kernel kernel-la -m 1G -nographic -smp 1 -drive file=sdcard-la.img,if=none,format=raw,id=x0 -device virtio-blk-pci,drive=x0 -no-reboot -device virtio-net-pci,netdev=net0 -netdev user,id=net0 -rtc base=utc 2>&1 | tee output_la.txt


vf2:
	$(MAKE) ARCH=riscv64 APP_FEATURES=vf2 MYPLAT=axplat-riscv64-visionfive2 BUS=mmio build

docker:
	docker run -it --rm --privileged \
	  --platform linux/amd64 \
	  -w /work \
	  -v $(CURDIR):/work \
	  docker.io/zhouzhouyi/os-contest:20260510 \
	  /bin/bash

.PHONY: all build run justrun debug disasm clean makeclean kernel-rv kernel-la debug-rv debug-la debug-rv-timerfd debug-la-timerfd contest-run-rv repair-sdcard-rv toolchain ensure-cargo-config prepare-sdcard-rv prepare-sdcard-la rv-shell la-shell docker
