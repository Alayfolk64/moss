#let os_name = "StarryOS"
#let team_name = "StarryOS 项目"
#let authors = "StarryOS contributors"
#let doc_date = "2026-06-09"

#set document(
  title: os_name + "操作系统内核设计文档",
  author: authors,
)

#set page(
  paper: "a4",
  margin: (top: 2.6cm, bottom: 2.6cm, left: 2.8cm, right: 2.8cm),
  numbering: "1",
  header: grid(
    columns: (1fr, auto),
    text(size: 9pt)[操作系统内核设计文档],
    text(size: 9pt)[#os_name],
  ),
)

#set text(
  font: ("Songti SC", "PingFang SC"),
  lang: "zh",
  size: 11pt,
)

#set par(justify: true, leading: 0.72em)
#set heading(numbering: "1.1")
#set list(indent: 1.5em)
#set enum(indent: 1.8em)
#show link: underline

#show raw.where(block: true): set text(font: ("Menlo", "Courier New"), size: 9pt)

#let title-page() = {
  pagebreak(weak: true)
  align(center)[
    #v(3cm)
    #text(size: 26pt, weight: "bold")[操作系统内核设计]
    #v(1.2cm)
    #text(size: 26pt, weight: "bold")[#os_name]
    #v(2cm)
    #text(size: 15pt)[#team_name]
    #v(0.6cm)
    #text(size: 15pt)[#authors]
    #v(0.6cm)
    #text(size: 15pt)[#doc_date]
  ]
  pagebreak()
}

#let todo-table(columns, rows) = {
  table(
    columns: columns,
    inset: 6pt,
    stroke: 0.5pt + gray,
    align: horizon,
    ..rows,
  )
}

#let evidence(rows) = {
  text(size: 9pt)[
    #table(
      columns: (1.7fr, 3.1fr, 5.2fr),
      inset: 5pt,
      stroke: 0.45pt + gray,
      align: horizon,
      [*模块*], [*路径*], [*代码证据*],
      ..rows,
    )
  ]
}
