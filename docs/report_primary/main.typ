// 封面信息：按需在这里替换即可。
#let report-title = [Moss]
#let report-subtitle = [设计文档]
#let team-name = [每天睡满八小时]
#let team-members = [吴勋、张可欣]
#let advisors = [马彬、钟福金]
#let report-date = [2026 年 6 月]

// 校徽预留：找到图片后，将 none 改为 image("pictrues/校徽.png", width: 13cm)。
#let school-logo = image("pictrues/loogoo.png", width: 13cm)

#let metadata-row(label, value) = {
  grid(
    columns: (auto, 6.5cm),
    column-gutter: 0.5cm,
    align: (left, center),
    [#label],
    [
      #block(
        width: 100%,
        inset: (bottom: 0.12em),
        stroke: (bottom: 0.7pt + black),
      )[
        #align(center)[#value]
      ]
    ],
  )
}

#let report-cover() = {
  set page(
    paper: "a4",
    margin: (top: 0cm, bottom: 2.5cm, x: 2.5cm),
  )
  set par(first-line-indent: 0em, justify: false)
  set text(font: ("Times New Roman", "Microsoft YaHei", "SimSun"), size: 12pt)

  // 页顶横线与校徽位置。
  rect(width: 100%, height: 2pt, fill: rgb("#2d2d2d"))
  v(2.2cm)
  align(center)[
    #if school-logo != none {
      school-logo
    } else {
      // 保持空白，供后续替换为校徽图片。
      block(width: 13cm, height: 3cm)[]
    }
  ]

  v(2.3cm)
  align(center)[
    #text(size: 30pt, weight: "bold", report-title)
  ]
  v(2.25cm)
  align(center)[
    #text(size: 20pt, report-subtitle)
  ]

  v(4.2cm)
  align(center)[
    #block(width: 10cm)[
      #set text(size: 14pt)
      #metadata-row([参赛队名], team-name)
      #v(1em)
      #metadata-row([队伍成员], team-members)
      #v(1em)
      #metadata-row([指导老师], advisors)
    ]
  ]

  v(1.2cm)
  align(center)[
    #text(size: 13pt, report-date)
  ]
}

#report-cover()
#pagebreak()

// 各章节文件会自行应用 template.typ 中的公共格式。
#show outline.entry: set text(font: "SimSun")
#show outline.entry.where(level: 1): set text(font: "Microsoft YaHei")

#align(center)[
  #text(font: "Microsoft YaHei", size: 18pt, weight: "regular")[目录]
]
#v(1.2em)

#outline(
  title: none,
  depth: 2,
)

#pagebreak()

// 章节顺序只在此文件维护，各章正文由对应作者独立编辑。
#include "ch1_概述.typ"
#include "ch2_进程与线程管理.typ"
#include "ch3_调度与同步机制.typ"
#include "ch4_内存管理.typ"
#include "ch5_文件系统.typ"
#include "ch6_信号机制.typ"
#include "ch7_网络.typ"
#include "ch8_QEMU虚拟硬件平台与设备驱动.typ"
#include "ch9_总结与展望.typ"
