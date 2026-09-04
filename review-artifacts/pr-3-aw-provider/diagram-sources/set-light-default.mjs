#!/usr/bin/env node

import fs from "node:fs";
import process from "node:process";

const files = process.argv.slice(2);
if (!files.length) {
  console.error("usage: set-light-default.mjs <archify.html> [...]");
  process.exit(2);
}

const replacements = [
  ['<html lang="zh-CN" data-theme="dark"', '<html lang="zh-CN" data-theme="light"'],
  [
    "theme = window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';",
    "theme = 'light';"
  ],
  [
    "return window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';",
    "return 'light';"
  ]
];

for (const file of files) {
  let html = fs.readFileSync(file, "utf8");
  for (const [before, after] of replacements) {
    const count = html.split(before).length - 1;
    if (count !== 1) {
      throw new Error(`${file}: expected one occurrence of ${before}, found ${count}`);
    }
    html = html.replace(before, after);
  }
  fs.writeFileSync(file, html);
  console.log(`set light as the default theme in ${file}`);
}
