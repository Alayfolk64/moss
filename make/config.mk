# Config generation

config_args := \
  defconfig.toml $(PLAT_CONFIG) $(EXTRA_CONFIG) \
  -w 'arch="$(ARCH)"' \
  -w 'platform="$(PLAT_NAME)"' \
  -o "$(OUT_CONFIG)"

ifneq ($(MEM),)
  config_args += -w 'plat.phys-memory-size=$(shell ./strtosz.py $(MEM))'
else
  MEM := $(shell $(AXCONFIG_GEN) $(PLAT_CONFIG) -r plat.phys-memory-size 2>/dev/null | tr -d _ | xargs printf "%dB")
endif

PLAT_MAX_CPU_NUM := $(shell $(AXCONFIG_GEN) $(PLAT_CONFIG) -r plat.max-cpu-num 2>/dev/null)
PLAT_CPU_NUM := $(shell $(AXCONFIG_GEN) $(PLAT_CONFIG) -r plat.cpu-num 2>/dev/null)
ifeq ($(PLAT_MAX_CPU_NUM),)
  ifeq ($(PLAT_CPU_NUM),)
    SMP_KEY :=
  else
    SMP_KEY := plat.cpu-num
  endif
else
  SMP_KEY := plat.max-cpu-num
endif

ifeq ($(SMP_KEY),)
  $(error "Neither `plat.max-cpu-num` nor `plat.cpu-num` is defined in the platform configuration file")
endif

# `CPU_CAPACITY` sizes compile-time per-CPU storage, while `SMP` controls how
# many CPUs QEMU exposes at runtime. Preserve the old behavior for callers that
# specify only `SMP`.
ifeq ($(CPU_CAPACITY),)
  ifneq ($(SMP),)
    CPU_CAPACITY := $(SMP)
  else
  ifeq ($(SMP_KEY),plat.max-cpu-num)
      CPU_CAPACITY := $(PLAT_MAX_CPU_NUM)
  else
      CPU_CAPACITY := $(PLAT_CPU_NUM)
  endif
  endif
endif

ifeq ($(CPU_CAPACITY),)
    $(error "Neither `plat.max-cpu-num` nor `plat.cpu-num` is defined in the platform configuration file, \
      this option must be specified even for platforms with runtime CPU detection.")
endif

config_args += -w '$(SMP_KEY)=$(CPU_CAPACITY)'

ifeq ($(SMP),)
  SMP := $(CPU_CAPACITY)
endif


define defconfig
  $(call run_cmd,$(AXCONFIG_GEN),$(config_args))
endef

ifeq ($(wildcard $(OUT_CONFIG)),)
  define oldconfig
    $(call defconfig)
  endef
else
  define oldconfig
    $(if $(filter "$(PLAT_NAME)",$(shell $(AXCONFIG_GEN) "$(OUT_CONFIG)" -r platform)),\
         $(call run_cmd,$(AXCONFIG_GEN),$(config_args) -c "$(OUT_CONFIG)"),\
         $(error "ARCH" or "MYPLAT" has been changed, please run "make defconfig" again))
  endef
endif
