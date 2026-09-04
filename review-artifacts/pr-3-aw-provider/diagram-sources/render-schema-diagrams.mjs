#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const sourcePath = process.argv[2];
const outputDirectory = process.argv[3];
if (!sourcePath || !outputDirectory) {
  console.error("usage: render-schema-diagrams.mjs <catalog.json> <output-dir>");
  process.exit(2);
}

const catalog = JSON.parse(fs.readFileSync(sourcePath, "utf8"));
fs.mkdirSync(outputDirectory, { recursive: true });

const escapeXml = (value) => String(value)
  .replaceAll("&", "&amp;")
  .replaceAll("<", "&lt;")
  .replaceAll(">", "&gt;")
  .replaceAll('"', "&quot;");

const wrap = (text, maxUnits) => {
  const lines = [];
  let line = "";
  let units = 0;
  const measure = (value) => [...value]
    .reduce((sum, character) => sum + (character.codePointAt(0) > 127 ? 2 : 1), 0);
  for (const character of text) {
    const width = character.codePointAt(0) > 127 ? 2 : 1;
    if (line && units + width > maxUnits) {
      const breakAt = line.lastIndexOf(" ");
      if (breakAt > 0) {
        lines.push(line.slice(0, breakAt));
        line = `${line.slice(breakAt + 1)}${character}`;
        units = measure(line);
      } else {
        lines.push(line);
        line = character;
        units = width;
      }
    } else {
      line += character;
      units += width;
    }
  }
  if (line) lines.push(line);
  return lines;
};

const textBlock = (lines, x, y, options = {}) => {
  const size = options.size ?? 22;
  const gap = options.gap ?? Math.round(size * 1.45);
  const fill = options.fill ?? "#243447";
  const weight = options.weight ?? 400;
  const anchor = options.anchor ?? "start";
  return `<text x="${x}" y="${y}" fill="${fill}" font-size="${size}" font-weight="${weight}" text-anchor="${anchor}" font-family="Inter, 'Noto Sans SC', system-ui, sans-serif">${lines.map((line, index) => `<tspan x="${x}" dy="${index === 0 ? 0 : gap}">${escapeXml(line)}</tspan>`).join("")}</text>`;
};

const listBlock = (items, x, y, widthUnits, color) => {
  let cursor = y;
  const chunks = [];
  for (const item of items) {
    const wrapped = wrap(item, widthUnits);
    chunks.push(`<circle cx="${x + 7}" cy="${cursor - 7}" r="4" fill="${color}"/>`);
    chunks.push(textBlock(wrapped, x + 22, cursor, { size: 18, gap: 27, fill: "#243447" }));
    cursor += wrapped.length * 27 + 14;
  }
  return { svg: chunks.join(""), height: cursor - y };
};

for (const schema of catalog.schemas) {
  const width = 1600;
  const headerHeight = 220;
  const groupX = 70;
  const groupW = 900;
  const reviewX = 1010;
  const reviewW = 520;
  const groupHeights = schema.groups.map((group) => 72 + group.items.reduce((sum, item) => sum + wrap(item, 72).length * 27 + 14, 0));
  const groupsHeight = groupHeights.reduce((sum, height) => sum + height, 0) + (schema.groups.length - 1) * 24;
  const strengthHeight = 86 + schema.strengths.reduce((sum, item) => sum + wrap(item, 39).length * 27 + 14, 0);
  const discussionHeight = 86 + schema.discussions.reduce((sum, item) => sum + wrap(item, 39).length * 27 + 14, 0);
  const bodyHeight = Math.max(groupsHeight, strengthHeight + discussionHeight + 24);
  const height = Math.max(1010, headerHeight + 24 + bodyHeight + 180);

  let groupY = headerHeight + 24;
  const groups = [];
  schema.groups.forEach((group, index) => {
    const boxH = groupHeights[index];
    const list = listBlock(group.items, groupX + 30, groupY + 82, 78, "#1689a7");
    groups.push(`<rect x="${groupX}" y="${groupY}" width="${groupW}" height="${boxH}" rx="18" fill="#ffffff" stroke="#c8d8e8" stroke-width="2"/>`);
    groups.push(textBlock([group.title], groupX + 30, groupY + 42, { size: 24, weight: 700, fill: "#126f8a" }));
    groups.push(`<line x1="${groupX + 30}" y1="${groupY + 58}" x2="${groupX + groupW - 30}" y2="${groupY + 58}" stroke="#d8e3ed"/>`);
    groups.push(list.svg);
    groupY += boxH + 24;
  });

  const reviewBox = (title, items, y, color, fill) => {
    const list = listBlock(items, reviewX + 26, y + 82, 48, color);
    const boxH = 86 + list.height;
    return {
      height: boxH,
      svg: `<rect x="${reviewX}" y="${y}" width="${reviewW}" height="${boxH}" rx="18" fill="${fill}" stroke="${color}" stroke-width="2"/>${textBlock([title], reviewX + 26, y + 42, { size: 24, weight: 700, fill: color })}<line x1="${reviewX + 26}" y1="${y + 58}" x2="${reviewX + reviewW - 26}" y2="${y + 58}" stroke="${color}" opacity="0.45"/>${list.svg}`
    };
  };

  const strength = reviewBox("当前合理", schema.strengths, headerHeight + 24, "#16845b", "#edf9f3");
  const discussion = reviewBox("需要讨论或修复", schema.discussions, headerHeight + 24 + strength.height + 24, "#b76308", "#fff7e8");
  const sourceLines = wrap(schema.source, 110);
  const copyLabel = schema.copies.length ? `物理副本：${schema.copies.join(", ")}` : "物理副本：无";
  const copyLines = wrap(copyLabel, 112);

  const svg = `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}" role="img" aria-labelledby="title desc">
  <title id="title">${escapeXml(schema.title)}</title>
  <desc id="desc">字段、语义、合理点和讨论点。审查固定到 ${catalog.review.head}。</desc>
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#fbfdff"/><stop offset="1" stop-color="#eaf2f9"/></linearGradient>
    <filter id="shadow" x="-20%" y="-20%" width="140%" height="140%"><feDropShadow dx="0" dy="8" stdDeviation="12" flood-color="#6d8498" flood-opacity="0.14"/></filter>
  </defs>
  <rect width="${width}" height="${height}" fill="url(#bg)"/>
  <rect x="38" y="32" width="1524" height="${height - 64}" rx="26" fill="#ffffff" fill-opacity="0.56" stroke="#c4d5e5" stroke-width="2"/>
  ${textBlock([schema.title], 70, 82, { size: 34, weight: 750, fill: "#17283b" })}
  ${textBlock([schema.kind], 70, 122, { size: 19, weight: 650, fill: "#087d9b" })}
  ${textBlock(wrap(schema.purpose, 126), 70, 165, { size: 20, gap: 28, fill: "#465d73" })}
  <line x1="70" y1="198" x2="1530" y2="198" stroke="#cbd9e6" stroke-width="2"/>
  <g filter="url(#shadow)">${groups.join("")}${strength.svg}${discussion.svg}</g>
  ${textBlock(sourceLines, 70, height - 100 - (copyLines.length * 21), { size: 14, gap: 21, fill: "#61758a" })}
  ${textBlock(copyLines, 70, height - 58, { size: 14, gap: 21, fill: "#61758a" })}
  ${textBlock([`PR head ${catalog.review.head.slice(0, 8)} · 绿色表示可保留，橙色表示冻结前需讨论`], 1530, height - 58, { size: 14, fill: "#61758a", anchor: "end" })}
</svg>\n`;

  fs.writeFileSync(path.join(outputDirectory, `${schema.id}.svg`), svg);
}

console.log(`rendered ${catalog.schemas.length} schema diagrams to ${outputDirectory}`);
