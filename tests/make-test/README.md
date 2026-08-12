# GNU Make 多进程编译测试

这个小工程分别编译 `main.c`、`add.c` 和 `mul.c`，然后链接为 `calc`。执行程序应输出：

```text
result = 42
```

逐项手动测试：

```sh
make --help

cd make-test
make clean
make
./calc

make clean
make -j4
./calc

sleep 1
touch add.c
make -j4
```

最后一次构建应只出现 `add.c` 的编译和 `calc` 的重新链接：

```text
[CC] add.c
[LD] calc
```

也可以在工程目录外直接运行完整检查：

```sh
sh make-test/run-tests.sh
```

若将它复制到 MossOS 根文件系统并放在 `/make-test`，则进入系统后执行：

```sh
cd /make-test
sh run-tests.sh
```
