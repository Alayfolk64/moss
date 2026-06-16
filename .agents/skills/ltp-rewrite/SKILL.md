---
name: ltp-rewrite
description: Rewrite Linux Test Project (LTP) syscall C test cases into simplified, standalone, human-readable C code. Use when asked to simplify, translate, annotate, or rewrite LTP tests, especially files under `syscall_code/` or `syscalls_code/`, removing the LTP test framework while writing rewritten outputs under `rewritten/` with Chinese intent and comments, while preserving the original observable output messages and format.
---

# LTP Rewrite

## Goal

Rewrite LTP syscall test cases into small standalone C programs that a person can read without knowing the LTP framework. Preserve the tested syscall behavior, expected result checks, and original observable output messages, but remove LTP-specific harness code.

## Input And Output

- Treat the repository root as the working directory unless the user specifies another root.
- Read original test code from the user-specified source directory. If the user says `syscall_code/` but the repository contains `syscalls_code/` instead, use `syscalls_code/`.
- Write rewritten code under the matching `rewritten/` directory. In this repository layout, that usually means `syscalls_code/<syscall>/rewritten/`.
- Never overwrite files in the source directory.
- If the user names specific files or syscalls, rewrite only those. Otherwise, inspect the source directory and choose the relevant C test files.
- Create `rewritten/` or nested output directories when missing.
- If a target file already exists in `rewritten/`, inspect it before editing and update only the requested rewritten test.

## Required Output Style

- At the top of every rewritten test file, before the code body, write a Chinese comment that explains the test case intent:
  - Which syscall or behavior is being tested.
  - What success or failure condition is expected.
  - Any important prerequisite, such as root permission, mount support, temporary files, invalid arguments, or expected `errno`.
- Add a Chinese comment above every constant, macro, global variable, helper function, and test function.
- Inside every function, add a Chinese comment above each logical block of code.
- Comments must explain purpose and test logic, not just restate C syntax.
- Prefer concise comments, but do not leave uncommented blocks.
- Keep comments in Chinese. Keep identifiers and syscall names in their original English form.
- Do not translate or rewrite the test's original printed messages. Chinese comments are required, but runtime output strings should preserve the original wording and format whenever the source test provides them.

## Rewrite Rules

- Remove LTP framework dependencies, including `tst_test`, `tst_res`, `tst_brk`, `tst_parse_opts`, `TEST()`, `TST_EXP_*`, `SAFE_*`, `TPASS`, `TFAIL`, `TBROK`, and LTP headers.
- Replace LTP assertions with direct C checks:
  - Use `if` statements for expected return values.
  - Check `errno` explicitly when a failure is expected.
  - Replace the printing framework with `printf`, `perror`, or `fprintf(stderr, ...)`, but preserve the original message text, placeholder order, errno text style, and newline behavior as closely as possible.
  - Return `0` only when all expected checks pass; return nonzero on failure or unsupported setup.
- Replace LTP setup and cleanup callbacks with ordinary helper functions or explicit code in `main`.
- Replace `SAFE_*` wrappers with direct system calls plus error handling.
- Keep the original syscall-focused behavior. Do not turn the test into a superficial demo.
- Preserve important variants from the original test, such as valid case, invalid pointer, invalid fd, permission failure, boundary value, or feature-not-supported path.
- If a behavior depends on kernel support, privileges, filesystem type, or architecture, detect it clearly. If the original test has a corresponding skip or failure message, preserve that output text; otherwise use a concise Chinese explanation before exiting with a nonzero or documented skip-like result.

## Output Preservation

- Treat runtime output as part of the test contract. The simplified code may change how messages are printed, but not what the original messages say.
- Keep original English strings from `tst_res`, `tst_brk`, `TST_EXP_*`, and related macros. Do not translate them into Chinese and do not polish the wording.
- Preserve format specifiers and argument order when converting an LTP message to `printf` or `fprintf`.
- Preserve silent behavior. If an original `TST_EXP_*_SILENT` path prints nothing on success, the rewritten success path should also stay silent unless the user asks for extra diagnostics.
- It is acceptable to add new output only for newly introduced environment checks, setup failures, or cleanup failures that did not have an original LTP message.

## Structure For Rewritten C Files

Use this structure unless the source test strongly suggests another readable shape:

1. File-level Chinese intent comment.
2. Standard C and Linux headers.
3. Commented constants, macros, and globals.
4. Small commented helper functions for setup, cleanup, and repeated checks.
5. A commented `main` function that runs the test steps in order.
6. Cleanup before every return path when resources were acquired.

Avoid rebuilding a mini test framework. A few helper functions are fine when they make the code shorter or safer.

## Workflow

1. Inspect the original LTP file and nearby helpers if needed.
2. Identify the syscall under test, setup requirements, test variants, expected return values, and expected `errno` values.
3. Draft a standalone C program that performs the same essential checks without LTP headers or macros.
4. Add Chinese comments before every file intent, declaration, helper, function, and logical code block.
5. Save the rewritten file under `rewritten/`.
6. Review the output for three things:
   - No `#include "tst_..."` or LTP-only symbols remain.
   - The test logic is still faithful to the original.
   - Runtime output messages and formats match the original test as closely as the removed LTP framework allows.
   - A reader can understand the intent from the Chinese comments alone.

## Comment Granularity

Use one comment per logical block. A logical block is usually:

- Preparing files, directories, buffers, flags, or syscall arguments.
- Executing the syscall under test.
- Checking the return value and `errno`.
- Reporting pass/fail status.
- Cleaning up resources.

Do not put comments only at the top of a function when the function has multiple logical steps.

## Example Comment Pattern

```c
// 本测例验证 openat 在目录文件描述符无效时会失败，并返回 EBADF。

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

// 用于触发 EBADF 的无效目录文件描述符。
#define BAD_DIRFD (-1)

// 入口函数按准备参数、执行 syscall、检查 errno 的顺序完成测例。
int main(void)
{
    // 准备一个相对路径，使 openat 必须依赖传入的目录文件描述符。
    const char *path = "missing-file";

    // 清空 errno，避免前一次库调用留下的值影响判断。
    errno = 0;

    // 调用 openat，预期它会因为 BAD_DIRFD 无效而失败。
    int fd = openat(BAD_DIRFD, path, O_RDONLY);

    // 检查 syscall 是否按预期失败，并确认 errno 是 EBADF。
    if (fd == -1 && errno == EBADF) {
        printf("openat() failed as expected\n");
        return 0;
    }

    // 报告非预期结果，输出字符串保持接近原测例的英文格式。
    fprintf(stderr, "openat() failed unexpectedly; expected: %d - %s\n",
            EBADF, strerror(EBADF));
    if (fd >= 0)
        close(fd);
    return 1;
}
```

## Final Response

After rewriting, summarize which files were created under `rewritten/` and mention whether compilation or execution was attempted. If not tested, say why.
