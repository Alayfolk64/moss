# Typst 报告工程

## 文件结构

- `main.typ`：整份报告入口，并控制章节顺序。
- `template.typ`：公共格式和章节初始化函数。
- `chN.typ`：各章正文，一位队员负责一个或多个独立文件。
- `pictrues/`：报告图片资源。

## 协作方式

1. 每位队员只修改自己负责的 `chN.typ`，避免同时编辑同一文件。
2. 图片使用带章节号的名称，例如 `图-2.4.1.svg`。
3. 新增章节后，在 `main.typ` 中按顺序添加 `#include "chN.typ"`。
4. 公共格式只在 `template.typ` 中修改，不在各章重复定义。
5. 编辑时直接打开并预览自己的 `chN.typ`；合并前统一编译 `main.typ`。

## 章节编号

每章开头引入模板并设置章号：

```typst
#import "template.typ": chapter-start, report-style

#show: report-style
#chapter-start(2)

= 进程与线程管理
```

表格、图片和代码题注使用统一的“章号-序号”格式。

## 编译

使用 Typst：

```powershell
typst compile main.typ report.pdf
```

使用 Tinymist：

```powershell
tinymist compile main.typ report.pdf
```

编辑章节时可直接编译对应文件：

```powershell
tinymist compile ch2.typ ch2.pdf
```
