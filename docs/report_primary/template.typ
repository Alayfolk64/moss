#let report-style(body) = {
  set text(
    font: ("Times New Roman", "SimSun"),
    size: 11pt,
    lang: "zh",
  )

  set page(
    paper: "a4",
    margin: (x: 2.5cm, y: 2.2cm),
  )

  set par(
    first-line-indent: (amount: 2em, all: true),
    justify: true,
    leading: 0.85em,
    spacing: 0.85em,
  )

  show raw.where(block: true): it => {
    block(
      fill: rgb("#f6f6f6"),
      inset: 1em,
      radius: 6pt,
      width: 100%,
      breakable: true,
      above: 0.35em,
      below: 0.35em,
      align(
        left,
        text(
          font: ("Consolas", "KaiTi"),
          size: 10pt,
          it,
        ),
      ),
    )
  }

  set heading(numbering: "1.1")

  set figure(numbering: (..nums) => {
    let chapter = counter(heading).get().first()
    numbering("1-1", chapter, ..nums)
  })

  show heading.where(level: 1): it => {
    pagebreak(weak: true)
    set align(center)
    set text(
      font: "Microsoft YaHei",
      size: 16pt,
      weight: "regular",
    )
    block[
      #if it.numbering != none [
        第#counter(heading).display()章 #it.body
      ] else [
        #it.body
      ]
    ]
    v(0.5em)
  }

  show heading.where(level: 2): it => {
    block(
      above: 1.1em,
      below: 1em,
      sticky: true,
      width: 100%,
    )[
      #set text(
        font: "Microsoft YaHei",
        size: 14pt,
        weight: "regular",
      )
      #counter(heading).display(it.numbering) #h(0.45em) #it.body
    ]
  }

  show heading.where(level: 3): it => {
    block(
      above: 1em,
      below: 1em,
      sticky: true,
      width: 100%,
    )[
      #set text(
        font: "Microsoft YaHei",
        size: 12pt,
        weight: "regular",
      )
      #counter(heading).display(it.numbering) #h(0.45em) #it.body
    ]
  }

  body
}

// 每章开头调用，例如第二章使用 #chapter-start(2)。
#let chapter-start(number) = {
  counter(heading).update(number - 1)
  counter(figure.where(kind: image)).update(0)
  counter(figure.where(kind: table)).update(0)
  counter(figure.where(kind: raw)).update(0)
}

