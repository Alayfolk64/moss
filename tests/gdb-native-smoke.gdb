set pagination off
set confirm off
set debuginfod enabled off
set startup-with-shell off
break native_marker
echo GDB_NATIVE_BREAKPOINT_SET\n
run
printf "GDB_NATIVE_BREAKPOINT_HIT pc=%p\n", $pc
bt
echo GDB_NATIVE_BACKTRACE_DONE\n
info proc mappings
echo GDB_NATIVE_PROC_MAPPINGS_DONE\n
info files
echo GDB_NATIVE_PROC_FILES_DONE\n
info auxv
echo GDB_NATIVE_PROC_AUXV_DONE\n
shell pid="$(pidof gdb-native-demo)" && echo GDB_NATIVE_PROC_STATUS_BEGIN && cat "/proc/$pid/status" && echo GDB_NATIVE_PROC_STATUS_DONE
info registers
echo GDB_NATIVE_REGS_DONE\n
x/4gx $sp
echo GDB_NATIVE_MEMORY_DONE\n
set $moss_before_pc = $pc
stepi
if $pc == $moss_before_pc
  echo GDB_NATIVE_STEPI_FAILED\n
  quit 1
end
printf "GDB_NATIVE_STEPI_DONE before=%p after=%p\n", $moss_before_pc, $pc
tbreak gdb-native-demo.c:24
continue
echo GDB_NATIVE_SOURCE_STEP_BEGIN\n
step
echo GDB_NATIVE_SOURCE_STEP_DONE\n
continue
echo GDB_NATIVE_SMOKE_DONE\n
quit 0
