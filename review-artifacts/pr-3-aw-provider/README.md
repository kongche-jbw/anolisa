# PR 3 AW Provider Review Artifacts

[中文版](README_zh.md)

This directory contains the read-only architecture review artifacts for
[casparant/anolisa PR 3](https://github.com/casparant/anolisa/pull/3).
The review is pinned to base
`8574ecb022ec9ffc68e1a71e30f2186b6ec81674` and head
`42d07649409ecd5bb023056b28545efbd9325ef2`.

These files are review evidence, not product documentation or an assertion that
the AW Provider path is production-ready.

## Reports

- [Full architecture review in Chinese](architecture-review_zh.md)
- [Executive architecture brief in Chinese](executive-brief_zh.md)
- [Provider principles and integration guide in Chinese](component-integration_zh.md)
- [Runtime examples and checkpoint boundary in Chinese](runtime-call-examples_zh.md)
- [Complete schema atlas in Chinese](schema-reference_zh.md)
- [Ready-to-paste PR review comment in Chinese](pr-review-comment_zh.md)
- [Agent Host POC comparison in Chinese](poc-comparison_zh.md)

## Interactive diagrams

- [Provider activation architecture](provider-effect-architecture.html)
- [Provider activation sequence](provider-effect-sequence.html), rendered as a
  Drafter engineering blueprint that keeps real fields and meanings inline
- [Agent Sec command inspection sequence](security-command-call.html), using six
  compact field stages for the request, finding, gate, and audit facts
- [Current checkpoint creation sequence](checkpoint-create-call.html), using six
  compact field stages for the CLI, socket, daemon, snapshot, and response

All three call sequences are self-contained Drafter HTML files with direct field
descriptions and step navigation. The Provider activation architecture remains a
self-contained Archify artifact and passed the nine showcase validation checks
without errors or warnings. The 14 deterministic light-theme schema SVGs are
stored under `images/schemas/`.
