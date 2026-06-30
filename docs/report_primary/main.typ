#import "template.typ": report-footer, report-header

// 封面信息：按需在这里替换即可。
#let report-title = [MossOS]
#let report-subtitle = [设计文档]
#let team-name = [每天睡满八小时]
#let team-members = [吴勋、张可欣]
#let advisors = [马彬、钟福金]
#let report-date = [2026 年 6 月]

#let report-cover() = {
  set page(
    paper: "a4",
    margin: (top: 0cm, bottom: 2.5cm, x: 2.5cm),
    header: none,
    footer: none,
  )
  set par(first-line-indent: 0em, justify: false)
  set text(font: ("Times New Roman", "Microsoft YaHei", "SimSun", "Songti SC"), size: 12pt)

  v(2.2cm)
  v(2.3cm)
  align(center)[
    #text(size: 30pt, weight: "bold", report-title)
  ]
  v(0.25cm)
  align(center)[
    #text(size: 20pt, report-subtitle)
  ]

  v(5.2cm)
  align(center)[
    #block(width: 10cm)[
      #set text(size: 14pt)
      #align(center)[
        #grid(
          columns: (auto, auto),
          column-gutter: 0.6em,
          row-gutter: 1em,
          align: (right, left),
          [*参赛院校：*], [*重庆邮电大学*],
          [*参赛队名：*], [*#team-name*],
          [*队伍成员：*], [*#team-members*],
        )
      ]
    ]
  ]

  v(2.2cm)
  align(center)[
    #text(size: 13pt, weight: "bold", report-date)
  ]
}

#report-cover()
#pagebreak()
#counter(page).update(1)
#set page(
  paper: "a4",
  margin: (top: 2.5cm, bottom: 2.2cm, x: 2.5cm),
  header-ascent: 1.2em,
  header: report-header(),
  footer: report-footer(numbering: "I"),
)

// 各章节文件会自行应用 template.typ 中的公共格式。
#show outline.entry: set text(font: ("SimSun", "Songti SC"))
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
#counter(page).update(1)
#set page(
  paper: "a4",
  margin: (top: 2.5cm, bottom: 2.2cm, x: 2.5cm),
  header-ascent: 1.2em,
  header: report-header(),
  footer: report-footer(numbering: "1"),
)

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
