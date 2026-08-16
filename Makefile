# Build Options
export ARCH := riscv64
export LOG := warn
export DWARF := n
export MODE := release
export MEMTRACK := n
export MM_STATS ?=n
export SHELL_INIT ?= n
export SITE_INIT ?= n
export RUSTUP_TOOLCHAIN ?= nightly-2025-05-20
RUSTUP ?= rustup
RUST_LLD ?= rust-lld
RUST_OBJCOPY ?= rust-objcopy
GDB ?= gdb-multiarch
LOONGARCH_CC ?=

# QEMU Options
export BLK := y
export NET := n
export VSOCK := n
export MEM := 8G
# Compile-time upper bound for per-CPU storage. QEMU's actual CPU count is
# discovered from its device tree at boot.
export CPU_CAPACITY ?= 256
export SMP ?= 8
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

ifeq ($(MM_STATS), y)
	APP_FEATURES += mm-stats
endif

ifeq ($(SHELL_INIT), y)
	APP_FEATURES += shell-init
endif

ifeq ($(SITE_INIT), y)
	APP_FEATURES += site-init
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
OUTPUT_RV := $(A)/docs/output_rv.txt
OUTPUT_LA := $(A)/docs/output_la.txt

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
		$(A)/moss-2k1000la.bin $(A)/.axconfig.ls2k1000.toml \
		$(A)/moss-2k1000la-shell.bin $(A)/.axconfig.ls2k1000-shell.toml \
		$(A)/moss-vf2.bin $(A)/.axconfig.vf2.toml \
		$(A)/kernel-rv.bin.o $(A)/kernel-la.bin.o $(A)/kernel-la.bin.S \
		$(A)/$(KERNEL_NAME)_riscv64-qemu-virt.elf $(A)/$(KERNEL_NAME)_riscv64-qemu-virt.bin \
		$(A)/$(KERNEL_NAME)_loongarch64-qemu-virt.elf $(A)/$(KERNEL_NAME)_loongarch64-qemu-virt.bin

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

all: 
	rm -rf target
	$(MAKE) kernel-rv
	$(MAKE) kernel-la

on_site:
	rm -rf target
	$(MAKE) SITE_INIT=y kernel-rv kernel-la kernel-vf2 kernel-la-2k1000

kernel-la-2k1000: toolchain
	rm -rf target
	$(MAKE) -C make \
		ARCH=loongarch64 \
		MYPLAT=axplat-loongarch64-ls2k1000 \
		PLAT_CONFIG=$(A)/vendor/axplat-loongarch64-ls2k1000/axconfig.toml \
		OUT_CONFIG=$(A)/.axconfig.ls2k1000.toml \
		APP_FEATURES="ls2k1000 $(if $(filter y,$(SITE_INIT)),site-init,)" \
		LOG=warn BUS=mmio BLK=n NET=n VSOCK=n MEM=2G \
		IP=$(if $(strip $(IP)),$(IP),192.168.88.2) \
		GW=$(if $(strip $(GW)),$(GW),192.168.88.1) \
		CPU_CAPACITY=2 SMP=2 defconfig build
	cp -f $(A)/$(KERNEL_NAME)_loongarch64-ls2k1000.bin $(A)/moss-2k1000la.bin
	@printf "    \033[36mLS2K1000LA image\033[0m: moss-2k1000la.bin\n"

kernel-la-2k1000-shell: toolchain
	rm -rf target
	$(MAKE) -C make \
		ARCH=loongarch64 \
		MYPLAT=axplat-loongarch64-ls2k1000 \
		PLAT_CONFIG=$(A)/vendor/axplat-loongarch64-ls2k1000/axconfig.toml \
		OUT_CONFIG=$(A)/.axconfig.ls2k1000-shell.toml \
		APP_FEATURES="ls2k1000 shell-init ls2k1000-shell-mode" \
		LOG=warn BUS=mmio BLK=n NET=n VSOCK=n MEM=2G \
		IP=$(if $(strip $(IP)),$(IP),192.168.88.2) \
		GW=$(if $(strip $(GW)),$(GW),192.168.88.1) \
		CPU_CAPACITY=2 SMP=2 defconfig build
	cp -f $(A)/$(KERNEL_NAME)_loongarch64-ls2k1000.bin $(A)/moss-2k1000la-shell.bin
	@printf "    \033[36mLS2K1000LA interactive image\033[0m: moss-2k1000la-shell.bin\n"

kernel-vf2: toolchain
	rm -rf target
	MOSS_BUILD_EPOCH=$${SOURCE_DATE_EPOCH:-$$(date +%s)} $(MAKE) -C make \
		ARCH=riscv64 \
		MYPLAT=axplat-riscv64-visionfive2 \
		PLAT_CONFIG=$(A)/vendor/axplat-riscv64-visionfive2/axconfig.toml \
		OUT_CONFIG=$(A)/.axconfig.vf2.toml \
		APP_FEATURES="vf2 $(if $(filter y,$(SITE_INIT)),site-init,shell-init)" \
		LOG=$(if $(strip $(LOG)),$(LOG),warn) BUS=mmio BLK=n NET=n VSOCK=n MEM=4G \
		IP=$(if $(strip $(IP)),$(IP),192.168.1.10) \
		GW=$(if $(strip $(GW)),$(GW),192.168.1.2) \
		CPU_CAPACITY=4 SMP=4 defconfig build
	cp -f $(A)/$(KERNEL_NAME)_visionfive2.bin $(A)/moss-vf2.bin
	@printf "    \033[36mVisionFive 2 image\033[0m: moss-vf2.bin\n"

kernel-rv: toolchain
	rm -rf target
	$(MAKE) ARCH=riscv64 MEM=$(MEM) SMP=$(SMP) CPU_CAPACITY=$(CPU_CAPACITY) build
ifeq ($(DWARF), y)
	cp -f $(A)/$(KERNEL_NAME)_riscv64-qemu-virt.elf $(A)/kernel-rv.dbg.elf
endif
	$(RUST_OBJCOPY) -I binary -O elf64-littleriscv -B riscv --rename-section .data=.text,alloc,load,readonly,code $(A)/$(KERNEL_NAME)_riscv64-qemu-virt.bin $(A)/kernel-rv.bin.o
	$(RUST_LLD) -flavor gnu -m elf64lriscv -T $(A)/make/kernel-rv-wrap.ld -o $(A)/kernel-rv $(A)/kernel-rv.bin.o
	rm -f $(A)/kernel-rv.bin.o $(A)/$(KERNEL_NAME)_riscv64-qemu-virt.bin
ifneq ($(DWARF), y)
	rm -f $(A)/$(KERNEL_NAME)_riscv64-qemu-virt.elf
endif

kernel-la: toolchain
	rm -rf target
	$(MAKE) ARCH=loongarch64 BUS=pci MEM=$(MEM) SMP=$(SMP) CPU_CAPACITY=$(CPU_CAPACITY) build
ifeq ($(DWARF), y)
	cp -f $(A)/$(KERNEL_NAME)_loongarch64-qemu-virt.elf $(A)/kernel-la.dbg.elf
endif
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
		'.incbin "$(A)/$(KERNEL_NAME)_loongarch64-qemu-virt.bin"' \
		'_binary_kernel_la_bin_end:' > $(A)/kernel-la.bin.S; \
	"$$LOONGARCH_CC_BIN" -c -o $(A)/kernel-la.bin.o $(A)/kernel-la.bin.S
	$(RUST_LLD) -flavor gnu -m elf64loongarch -T $(A)/make/kernel-la-wrap.ld -o $(A)/kernel-la $(A)/kernel-la.bin.o
	rm -f $(A)/kernel-la.bin.o $(A)/kernel-la.bin.S $(A)/$(KERNEL_NAME)_loongarch64-qemu-virt.bin
ifneq ($(DWARF), y)
	rm -f $(A)/$(KERNEL_NAME)_loongarch64-qemu-virt.elf
endif

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
		-netdev user,id=net -rtc base=utc -S -s 2>&1 | tee $(OUTPUT_RV)

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
		-rtc base=utc -S -s 2>&1 | tee $(OUTPUT_LA)

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

run_rv: kernel-rv prepare-sdcard-rv
	qemu-system-riscv64 -machine virt -kernel kernel-rv -m $(MEM) -nographic -smp 1 -bios default \
		-drive file=sdcard-rv.img,if=none,format=raw,id=x0 \
		-device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
		-no-reboot -device virtio-net-device,netdev=net \
		-netdev user,id=net -rtc base=utc 2>&1 | tee $(OUTPUT_RV)

run_la: kernel-la prepare-sdcard-la
	qemu-system-loongarch64 -kernel kernel-la -m $(MEM) -nographic -smp 1 \
		-drive file=sdcard-la.img,if=none,format=raw,id=x0 \
		-device virtio-blk-pci,drive=x0 -no-reboot \
		-device virtio-net-pci,netdev=net0 -netdev user,id=net0 \
		-rtc base=utc 2>&1 | tee $(OUTPUT_LA)

run-rv: run_rv
run-la: run_la

rv:
	$(MAKE) ARCH=riscv64 run

shell_rv:
	$(MAKE) ARCH=riscv64 SHELL_INIT=y kernel-rv
	@img="$(A)/sdcard-rv.img"; \
	tmp_img="$(A)/tmp-rv-shell.img"; \
	if [ ! -f "$$img" ]; then \
		$(MAKE) --no-print-directory prepare-sdcard-rv; \
	fi; \
	rm -f "$$tmp_img"; \
	cp -f "$$img" "$$tmp_img"; \
	qemu-system-riscv64 -machine virt -kernel kernel-rv -m $(MEM) -nographic -smp 1 -bios default -drive file=sdcard-rv.img,if=none,format=raw,id=x0 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -snapshot -no-reboot -device virtio-net-device,netdev=net -netdev user,id=net -rtc base=utc 2>&1 | tee $(OUTPUT_RV)

rv-shell: shell_rv


la:
	$(MAKE) ARCH=loongarch64 BUS=pci run

shell_la:
	$(MAKE) ARCH=loongarch64 BUS=pci SHELL_INIT=y kernel-la
	@img="$(A)/sdcard-la.img"; \
	tmp_img="$(A)/tmp-la-shell.img"; \
	if [ ! -f "$$img" ]; then \
		$(MAKE) --no-print-directory prepare-sdcard-la; \
	fi; \
	rm -f "$$tmp_img"; \
	cp -f "$$img" "$$tmp_img"; \
	qemu-system-loongarch64 -kernel kernel-la -m $(MEM) -nographic -smp 1 -drive file=tmp-la-shell.img,if=none,format=raw,id=x0 -device virtio-blk-pci,drive=x0 -snapshot -no-reboot -device virtio-net-pci,netdev=net0 -netdev user,id=net0 -rtc base=utc 2>&1 | tee $(OUTPUT_LA)

la-shell: shell_la


vf2: kernel-vf2

docker:
	docker run -it --rm --privileged \
	  --platform linux/amd64 \
	  -w /work \
	  -v $(CURDIR):/work \
	  docker.io/zhouzhouyi/os-contest:20260510 \
	  /bin/bash

.PHONY: all on_site build run justrun debug disasm clean makeclean kernel-rv kernel-la kernel-la-2k1000 kernel-la-2k1000-shell kernel-vf2 vf2 debug-rv debug-la contest-run-rv repair-sdcard-rv toolchain ensure-cargo-config prepare-sdcard-rv prepare-sdcard-la run_rv run_la run-rv run-la shell_rv shell_la rv-shell la-shell docker
