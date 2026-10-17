"use strict";
(() => {
  const snapshot = JSON.parse(document.getElementById("snapshot").textContent);
  const byId = id => document.getElementById(id);
  const text = (id, value) => { byId(id).textContent = value; };
  const make = (tag, value, className) => {
    const element = document.createElement(tag);
    if (value !== undefined) element.textContent = value;
    if (className) element.className = className;
    return element;
  };
  const size = value => {
    if (value === null || value === undefined) return "Unavailable";
    const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let unit = 0;
    while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit++; }
    return `${value.toLocaleString(undefined, { maximumFractionDigits: unit ? 1 : 0 })} ${units[unit]}`;
  };
  const root = snapshot.inventory;
  const parents = new Map();
  const index = (node, parent) => { parents.set(node, parent); node.children.forEach(child => index(child, node)); };
  index(root, null);
  let current = root;
  let limit = 100;
  const categoryRows = snapshot.recommendations.categories.flatMap(category => category.findings.map(finding => ({ ...finding, category: category.category })));
  const color = node => {
    const row = categoryRows.find(row => node.path === row.path || node.path.startsWith(`${row.path}/`));
    if (row?.category === "agents" || /\/(\.codex|\.claude|\.pi)(\/|$)/.test(node.path)) return "agents";
    if (["artifacts", "node-modules", "xcode"].includes(row?.category)) return "build";
    if (row?.kind === "review") return "review";
    return row ? "preview" : "other";
  };
  text("root-path", snapshot.root);
  text("report-path", snapshot.output_file);
  text("captured", new Date(snapshot.generated_unix_seconds * 1000).toLocaleString());
  byId("captured").dateTime = new Date(snapshot.generated_unix_seconds * 1000).toISOString();
  text("mapped", `${snapshot.coverage.partial ? "≥ " : ""}${size(root.bytes)}`);
  text("coverage-label", snapshot.coverage.partial ? "Partial lower bound · some entries unavailable" : "Files and symlinks · logical lengths");
  text("available", size(snapshot.volume.available_bytes));
  text("capacity", snapshot.volume.capacity_bytes === null ? "Volume metrics unavailable; see diagnostics" : `of ${size(snapshot.volume.capacity_bytes)} volume capacity`);
  text("candidates", snapshot.recommendations.categories.length.toLocaleString());
  text("bounds", `Overview: up to ${snapshot.coverage.max_depth} levels and ${snapshot.coverage.max_nodes.toLocaleString()} entries. ${snapshot.coverage.summarized_entries.toLocaleString()} entries have summarized detail; their measured bytes remain included. Symlinks are not followed; foreign filesystems are not traversed.`);
  if (snapshot.errors.length) {
    byId("banner").hidden = false;
    const notices = [snapshot.coverage.partial ? "Map is a partial lower bound." : "Map measurement is complete."];
    if (snapshot.volume.error) notices.push("Volume data unavailable.");
    if (snapshot.recommendations.errors.length) {
      const failures = new Set(snapshot.recommendations.errors.map(error => error.split(":", 1)[0]));
      notices.push(`${failures.size} cleanup ${failures.size === 1 ? "category unavailable" : "categories unavailable"}.`);
    }
    byId("banner").append(make("span", notices.join(" ")));
    const details = make("button", "Show diagnostics"); details.type = "button";
    details.addEventListener("click", () => { byId("diagnostics-disclosure").open = true; byId("diagnostics-disclosure").querySelector("summary").focus(); });
    byId("banner").append(details);
  }
  const diagnostics = [...snapshot.coverage.errors, ...(snapshot.volume.error ? [snapshot.volume.error] : []), ...snapshot.recommendations.errors, snapshot.recommendations.roots_note, ...snapshot.recommendations.diagnostics];
  if (snapshot.errors.length) diagnostics.unshift("Resolve the access or tool errors below, then rerun devtrim dashboard with the same path. Category preview commands also retry their own probes.");
  if (snapshot.coverage.issue_count > snapshot.coverage.errors.length) diagnostics.push(`${snapshot.coverage.issue_count - snapshot.coverage.errors.length} additional inventory issues; individual diagnostics are limited to 256.`);
  diagnostics.forEach(message => byId("diagnostics").append(make("p", message)));
  const info = node => {
    text("detail-path", node.path);
    text("detail-info", `${node.kind} · ${node.partial ? "at least " : ""}${size(node.bytes)} logical bytes${node.partial ? " · partial measurement" : ""}${node.summarized_entries ? ` · ${node.summarized_entries.toLocaleString()} entries summarized here (${size(node.summarized_bytes)})` : ""}. Inventory only; no deletion action is attached to this item.`);
  };
  const navigate = node => {
    info(node);
    if (node.kind !== "directory") return;
    current = node;
    limit = 100;
    byId("search").value = "";
    render();
    byId("breadcrumbs").lastElementChild?.querySelector("button").focus();
  };
  const svgNode = (tag, attributes) => {
    const element = document.createElementNS("http://www.w3.org/2000/svg", tag);
    Object.entries(attributes).forEach(([key, value]) => element.setAttribute(key, String(value)));
    return element;
  };
  // A balanced binary partition preserves area without thin slice-and-dice strips.
  const partition = (items, x, y, width, height, draw) => {
    if (!items.length) return;
    if (items.length === 1) { draw(items[0], x, y, width, height); return; }
    const total = items.reduce((sum, item) => sum + item.bytes, 0);
    let split = 1, first = items[0].bytes;
    while (split < items.length - 1 && first + items[split].bytes / 2 < total / 2) first += items[split++].bytes;
    const fraction = first / total;
    if (width >= height) {
      partition(items.slice(0, split), x, y, width * fraction, height, draw);
      partition(items.slice(split), x + width * fraction, y, width * (1 - fraction), height, draw);
    } else {
      partition(items.slice(0, split), x, y, width, height * fraction, draw);
      partition(items.slice(split), x, y + height * fraction, width, height * (1 - fraction), draw);
    }
  };
  // Bounds include the aggregate tile. No displayed child's bytes disappear
  // when a sibling or a deeper measured subtree is summarized.
  const visibleItems = (node, budget) => {
    const children = node.children.filter(child => child.bytes > 0).slice().sort((left, right) => right.bytes - left.bytes || left.path.localeCompare(right.path));
    if (children.length <= budget && !node.summarized_bytes) return children;
    const shown = children.slice(0, Math.max(0, budget - 1));
    const other = node.summarized_bytes + children.slice(shown.length).reduce((sum, child) => sum + child.bytes, 0);
    if (other) shown.push({ name: "Other measured entries", path: node.path, bytes: other, summarized: true, owner: node });
    return shown;
  };
  const shorten = (name, width) => {
    const characters = Math.max(1, Math.floor(width / 7.5));
    return name.length > characters ? `${name.slice(0, Math.max(0, characters - 1))}…` : name;
  };
  const tileButton = (node, x, y, width, height, heading = false) => {
    const measured = `${node.partial ? "≥ " : ""}${size(node.bytes)}`;
    const tile = svgNode("g", { class: `tile ${heading ? "parent-heading other" : node.summarized ? "other" : color(node)}`, role: "button", tabindex: 0, "aria-label": `${node.name}, ${node.partial ? "at least " : ""}${size(node.bytes)}${node.summarized ? ", summarized detail" : `, ${node.kind}`}` });
    tile.append(svgNode("rect", { x, y, width, height, rx: 3 }));
    const title = svgNode("title", {}); title.textContent = `${node.path}\n${measured}`; tile.append(title);
    if (heading) {
      const label = measured;
      const name = svgNode("text", { x: x + 8, y: y + 19 }); name.textContent = shorten(node.name, width - label.length * 7 - 30); tile.append(name);
      const value = svgNode("text", { x: x + width - 8, y: y + 19, "text-anchor": "end", class: "tile-size" }); value.textContent = label; tile.append(value);
    } else if (width >= 75 && height >= 42) {
      const name = svgNode("text", { x: x + 8, y: y + 22 }); name.textContent = shorten(node.name, width - 18); tile.append(name);
      if (height >= 62 && width >= 95) { const label = svgNode("text", { x: x + 8, y: y + 42, class: "tile-size" }); label.textContent = measured; tile.append(label); }
    }
    const activate = () => {
      if (node.summarized) { info(node.owner); text("detail-info", `${size(node.bytes)} of measured entries grouped in this map. Retained children remain available in the folder list; summarized descendants need a new dashboard rooted closer to them.`); }
      else navigate(node);
    };
    tile.addEventListener("click", event => { event.stopPropagation(); activate(); });
    tile.addEventListener("keydown", event => { if (["Enter", " "].includes(event.key)) { event.preventDefault(); event.stopPropagation(); activate(); } });
    return tile;
  };
  const drawMap = () => {
    const map = byId("map"); map.replaceChildren();
    const shown = visibleItems(current, 60);
    if (!shown.length) { map.append(make("p", current.partial ? "No readable bytes to map. See coverage diagnostics." : "No measured bytes in this folder. Empty and zero-byte entries remain in the list.", "empty")); return; }
    const mapWidth = Math.max(1, map.clientWidth);
    const mapHeight = Math.round(Math.max(256, Math.min(440, mapWidth * .6)));
    const svg = svgNode("svg", { width: mapWidth, height: mapHeight, viewBox: `0 0 ${mapWidth} ${mapHeight}`, role: "group", "aria-label": "Logical size treemap with one nested folder level" });
    let nestedBudget = 180;
    partition(shown, 0, 0, mapWidth, mapHeight, (node, x, y, width, height) => {
      if (node.kind !== "directory" || width < 170 || height < 115 || !nestedBudget) { svg.append(tileButton(node, x, y, width, height)); return; }
      const children = visibleItems(node, Math.min(16, nestedBudget));
      if (!children.length) { svg.append(tileButton(node, x, y, width, height)); return; }
      nestedBudget -= children.length;
      const folder = svgNode("g", { class: "folder-group", role: "group", "aria-label": `${node.name} folder, ${node.partial ? "at least " : ""}${size(node.bytes)}` });
      folder.append(svgNode("rect", { x, y, width, height, rx: 3 }));
      // Only the heading navigates to the parent; child buttons are siblings,
      // never nested inside another button or its click handler.
      folder.append(tileButton(node, x + 2, y + 2, width - 4, 28, true));
      partition(children, x + 3, y + 32, width - 6, height - 35, (child, childX, childY, childWidth, childHeight) => folder.append(tileButton(child, childX, childY, childWidth, childHeight)));
      svg.append(folder);
    });
    map.append(svg);
  };
  const renderList = () => {
    const query = byId("search").value.trim().toLocaleLowerCase();
    let entries = current.children.slice();
    if (query) {
      entries = [];
      const visit = node => node.children.forEach(child => { if (child.name.toLocaleLowerCase().includes(query) || child.path.toLocaleLowerCase().includes(query)) entries.push(child); visit(child); });
      visit(current);
    }
    entries.sort(byId("sort").value === "name" ? (left, right) => left.name.localeCompare(right.name) || left.path.localeCompare(right.path) : (left, right) => right.bytes - left.bytes || left.path.localeCompare(right.path));
    const list = byId("folders"); list.replaceChildren();
    text("list-count", `${entries.length.toLocaleString()} ${query ? "matching retained entries in this subtree" : "retained entries in this folder"}${current.summarized_entries ? " · additional measured detail summarized" : ""}`);
    entries.slice(0, limit).forEach(node => {
      const item = make("li");
      const button = make("button", undefined, "folder-row"); button.type = "button";
      const name = make("span", node.name, "folder-name"); name.append(make("span", `${node.kind}${node.partial ? " · partial" : ""}`, `folder-type ${color(node)}`));
      button.append(name, make("span", `${node.partial ? "≥ " : ""}${size(node.bytes)}`, "folder-size"), make("span", node.path, "folder-path"));
      button.addEventListener("click", () => navigate(node)); item.append(button); list.append(item);
    });
    if (!entries.length) list.append(make("li", query ? "No matching retained entries. Search covers the overview, not summarized descendants." : current.summarized_entries ? "Deeper detail is summarized. Generate a new dashboard rooted here for a closer view." : "This folder has no retained children.", "empty"));
    byId("more").hidden = entries.length <= limit;
  };
  const render = () => {
    const crumbs = []; let ancestor = current;
    while (ancestor) { crumbs.unshift(ancestor); ancestor = parents.get(ancestor); }
    byId("breadcrumbs").replaceChildren();
    crumbs.forEach(node => { const item = make("li"); const button = make("button", node.name); button.type = "button"; button.title = node.path; if (node === current) button.setAttribute("aria-current", "location"); button.addEventListener("click", () => navigate(node)); item.append(button); byId("breadcrumbs").append(item); });
    byId("back").disabled = !parents.get(current);
    text("current-name", current.name); text("current-size", `${current.partial ? "≥ " : ""}${size(current.bytes)}`);
    drawMap(); renderList(); info(current);
  };
  byId("back").addEventListener("click", () => { const parent = parents.get(current); if (parent) navigate(parent); });
  byId("search").addEventListener("input", () => { limit = 100; renderList(); });
  byId("sort").addEventListener("change", renderList);
  byId("more").addEventListener("click", () => { limit += 100; renderList(); });
  const recommendations = byId("recommendations");
  snapshot.recommendations.categories.forEach(category => {
    const section = make("section", undefined, "category"); section.append(make("h3", category.category.replaceAll("-", " ")));
    const list = make("ul");
    category.findings.forEach(row => {
      const item = make("li"); item.append(make("strong", row.label), make("p", `${row.kind === "review" ? "Review" : "Category preview"} · ${size(row.bytes)}`), make("p", row.path, "full-path"));
      const details = make("details"); details.append(make("summary", "Why this appears"), make("p", row.note)); item.append(details); list.append(item);
    });
    section.append(list);
    const field = make("input"); field.type = "text"; field.readOnly = true; field.value = category.preview_command; field.setAttribute("aria-label", `${category.category} preview command`);
    const button = make("button", "Copy preview command"); button.type = "button";
    button.addEventListener("click", async () => {
      try { await navigator.clipboard.writeText(category.preview_command); text("copy-status", `Copied ${category.preview_command}. Run it in your terminal for a fresh preview.`); }
      catch { field.focus(); field.select(); text("copy-status", "Clipboard unavailable. The preview command is selected; copy it from the field."); }
    });
    section.append(field, button); recommendations.append(section);
  });
  if (!snapshot.recommendations.categories.length) recommendations.append(make("p", snapshot.recommendations.errors.length ? "Category probes are incomplete. No matching rows are available; this does not establish that no candidates exist." : "No category findings inside this map. Everything shown in the explorer remains inventory only."));
  render();
  let lastWidth = byId("map").clientWidth;
  new ResizeObserver(() => {
    const width = byId("map").clientWidth;
    if (width !== lastWidth) { lastWidth = width; drawMap(); }
  }).observe(byId("map"));
})();
