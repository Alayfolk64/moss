# Build Options
export ARCH := riscv64
export LOG := warn
export DWARF := y
export MEMTRACK := n

# QEMU Options
export BLK := y
export NET := y
export VSOCK := n
export MEM := 1G
export ICOUNT := n
export BUS := mmio

# Generated Options
export A := $(PWD)
export NO_AXSTD := y
export AX_LIB := axfeat
export APP_FEATURES := qemu

ifeq ($(MEMTRACK), y)
	APP_FEATURES += starry-api/memtrack
endif

default: build

KERNEL_NAME := $(notdir $(A))

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

defconfig justrun:
	@$(MAKE) -C make $@

makeclean:
	@$(MAKE) -C make makeclean
	@rm -f $(A)/kernel-rv $(A)/kernel-la

clean: makeclean
	@:

build run debug disasm: defconfig
	@$(MAKE) -C make $@

all: kernel-rv kernel-la

kernel-rv:
	$(MAKE) ARCH=riscv64 build
	cp -f $(A)/$(KERNEL_NAME)_riscv64-qemu-virt.elf $(A)/kernel-rv

kernel-la:
	$(MAKE) ARCH=loongarch64 build
	cp -f $(A)/$(KERNEL_NAME)_loongarch64-qemu-virt.elf $(A)/kernel-la

contest-run-rv: defconfig
	@test -f "$(A)/sdcard-rv.img" || (echo "missing image: $(A)/sdcard-rv.img" && exit 1)
	@$(MAKE) -C make run ARCH=riscv64 BLK=y NET=y BUS=mmio DISK_IMG=$(A)/sdcard-rv.img

ci-test:
	./scripts/ci-test.py $(ARCH)

# Aliases
rv:
	$(MAKE) ARCH=riscv64 run

la:
	$(MAKE) ARCH=loongarch64 run

vf2:
	$(MAKE) ARCH=riscv64 APP_FEATURES=vf2 MYPLAT=axplat-riscv64-visionfive2 BUS=mmio build

.PHONY: all build run justrun debug disasm clean makeclean kernel-rv kernel-la contest-run-rv
