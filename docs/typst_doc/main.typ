#import "template.typ": *

#title-page()

= 修订记录

#table(
  columns: (1fr, 1.5fr, 1.5fr, 4fr),
  inset: 6pt,
  stroke: 0.5pt + gray,
  [*版本*], [*日期*], [*作者*], [*修改内容*],
  [v1.0], [2026-06-09], [StarryOS contributors], [基于当前 StarryOS 源码整理设计文档],
)

#pagebreak()
#outline(title: "目录", depth: 3)
#pagebreak()

#include "chapters/01-overview.typ"
#include "chapters/02-system-architecture.typ"
#include "chapters/03-arch-platform.typ"
#include "chapters/04-userspace.typ"
#include "chapters/06-memory.typ"
#include "chapters/07-process-scheduler.typ"
#include "chapters/08-filesystem.typ"
#include "chapters/05-gui.typ"
#include "chapters/09-signal.typ"
#include "chapters/10-network.typ"
#include "chapters/11-drivers.typ"
#include "chapters/12-extensions.typ"
#include "chapters/13-testing.typ"
#include "chapters/14-summary.typ"

#include "appendices/a-build-run.typ"
#include "appendices/b-syscalls.typ"
#include "appendices/c-data-structures.typ"
#include "appendices/d-test-logs.typ"
