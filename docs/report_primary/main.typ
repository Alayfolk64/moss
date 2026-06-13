#import "template.typ": report-style

#report-style[
  // 封面和摘要稳定后，可在目录前通过 include 引入。
  #outline(
    title: [目录],
    depth: 2,
  )

  #pagebreak()
]

// 章节顺序只在此文件维护，各章正文由对应作者独立编辑。
// #include "ch1.typ"
#include "ch2.typ"
// #include "ch3.typ"
