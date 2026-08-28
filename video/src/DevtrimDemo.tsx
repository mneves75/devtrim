import React from "react";
import { AbsoluteFill, interpolate, useCurrentFrame } from "remotion";

const BG = "#0a0a0a";
const INK = "#f4f1e9";
const GREEN = "#c8ff38";
const AMBER = "#ffc53d";
const RED = "#ff6b57";
const CYAN = "#7fd7d0";
const BLUE = "#7aa2f7";
const DIM = "#7c7a74";
const MUTED = "#aaa8a1";
const LINE = "rgba(244,241,233,0.22)";

const COLS = 100;
const FONT = 28;
const LH = 38;

type Cell = { t: string; c?: string; b?: boolean };
type Row = Cell[];

const width = (row: Row) => row.reduce((n, c) => n + c.t.length, 0);

const pad = (row: Row, w: number): Row => {
  const diff = w - width(row);
  return diff > 0 ? [...row, { t: " ".repeat(diff) }] : row;
};

/** Ratatui `Block::bordered().title(...)` with a fixed inner area. */
const boxed = (title: string, w: number, h: number, lines: Row[]): Row[] => {
  const top: Row = [
    { t: "┌", c: LINE },
    { t: title },
    { t: "─".repeat(Math.max(0, w - 2 - title.length)), c: LINE },
    { t: "┐", c: LINE },
  ];
  const bottom: Row = [{ t: `└${"─".repeat(w - 2)}┘`, c: LINE }];
  const inner: Row[] = [];
  for (let i = 0; i < h - 2; i++) {
    const l = lines[i] ?? [];
    inner.push([{ t: "│", c: LINE }, ...pad(l, w - 2), { t: "│", c: LINE }]);
  }
  return [top, ...inner, bottom];
};

/** Word wrap, matching Ratatui `Wrap { trim: false }`. */
const wrap = (text: string, w: number): string[] =>
  text.split(" ").reduce<string[]>((lines, word) => {
    const last = lines[lines.length - 1];
    if (last !== undefined && last.length + 1 + word.length <= w) {
      lines[lines.length - 1] = `${last} ${word}`;
    } else {
      lines.push(word);
    }
    return lines;
  }, []);

const hjoin = (a: Row[], b: Row[]): Row[] =>
  a.map((row, i) => [...row, ...(b[i] ?? [])]);

const Screen: React.FC<{ rows: Row[] }> = ({ rows }) => (
  <>
    {rows.map((row, i) => (
      <div key={i} style={{ height: LH, whiteSpace: "pre" }}>
        {row.map((cell, j) => (
          <span
            key={j}
            style={{ color: cell.c ?? INK, fontWeight: cell.b ? 700 : 400 }}
          >
            {cell.t}
          </span>
        ))}
      </div>
    ))}
  </>
);

const MENU = [
  ["1", "Scan everything", "READ-ONLY"],
  ["2", "Caches", "PREVIEW"],
  ["3", "node_modules", "PREVIEW"],
  ["4", "Build artifacts", "PREVIEW"],
  ["5", "Simulators", "PREVIEW"],
  ["6", "Xcode", "PREVIEW"],
  ["7", "Docker", "PREVIEW"],
  ["8", "Swift toolchains", "PREVIEW"],
  ["9", "Agent leftovers", "READ-ONLY"],
  ["i", "iCloud status", "READ-ONLY"],
  ["0", "Empty Trash", "PERMANENT"],
];

const markerColor = (m: string) =>
  m === "PERMANENT" ? RED : m === "READ-ONLY" ? BLUE : GREEN;

const DETAIL: Record<number, string[]> = {
  0: [
    "Scan everything",
    "Read-only report across every cleanup category.",
    "No mutation is available from this screen.",
  ],
  1: [
    "Caches",
    "Regenerable package and model download caches.",
    "Selecting this operation scans first. Apply is a separate, explicit step.",
  ],
};

const FINDINGS = [
  [
    "3",
    "huggingface model cache",
    "21.0 GB",
    "~/.cache/huggingface",
    "Re-downloads on demand.",
  ],
  [
    "2",
    "npm cache",
    "3.4 GB",
    "~/.npm/_cacache",
    "Rebuilt by the next install.",
  ],
  [
    "2",
    "Homebrew downloads",
    "1.9 GB",
    "~/Library/Caches/Homebrew",
    "Re-fetched on demand.",
  ],
  ["2", "uv cache", "1.2 GB", "~/.cache/uv", "Re-downloads on demand."],
];

const dangerColor = (d: string) =>
  Number(d) >= 6 ? RED : Number(d) >= 3 ? AMBER : GREEN;

const NOTICE = [
  "Applying this plan can delete data. devtrim is provided AS IS, without",
  "warranties; you assume the risk for the exact targets shown. Keep backups",
  "and grant macOS permissions manually only when you understand the request.",
];

const header = (operation: string): Row[] =>
  boxed(" measure · classify · trim ", COLS, 3, [
    [
      { t: " devtrim ", c: GREEN, b: true },
      { t: "v0.6.2  ", c: DIM },
      { t: operation },
    ],
  ]);

const footer = (keys: string, status: string): Row[] =>
  boxed("", COLS, 4, [[{ t: keys, c: CYAN }], [{ t: status, c: AMBER }]]);

const menuBody = (selected: number): Row[] => {
  const list = MENU.map(
    ([key, label, marker], i): Row => [
      { t: i === selected ? "▶ " : "  ", c: GREEN, b: true },
      { t: ` ${key} `, c: DIM },
      { t: label, c: i === selected ? GREEN : INK, b: i === selected },
      { t: `  ${marker}`, c: markerColor(marker) },
    ],
  );
  const d = DETAIL[selected] ?? DETAIL[0];
  const detail: Row[] = [
    [{ t: d[0], c: GREEN, b: true }],
    [],
    ...wrap(d[1], 52).map((t): Row => [{ t }]),
    [],
    ...wrap(d[2], 52).map((t): Row => [{ t, c: AMBER }]),
    [],
    ...wrap(
      "↑/↓ or j/k navigate · Enter opens · menu key opens directly",
      52,
    ).map((t): Row => [{ t }]),
  ];
  return hjoin(
    boxed(" Operations ", 46, 17, list),
    boxed(" Selected ", 54, 17, detail),
  );
};

const resultsBody = (): Row[] => {
  const lines: Row[] = [];
  FINDINGS.forEach(([danger, label, size, path, note], i) => {
    lines.push([
      {
        t: `${String(i + 1).padStart(2)}. danger-${danger}  `,
        c: dangerColor(danger),
        b: true,
      },
      { t: label, b: true },
      { t: `  ${size}  TRASH`, c: CYAN },
    ]);
    lines.push([{ t: path }]);
    lines.push([{ t: note, c: DIM }]);
    lines.push([]);
  });
  return boxed(
    " Preview · 4 finding(s) · 27.5 GB actionable · danger-3 · TRASH-FIRST ",
    COLS,
    17,
    lines,
  );
};

const outcomeBody = (): Row[] =>
  boxed(" Apply outcome ", COLS, 17, [
    [
      {
        t: "caches · 4 item(s) · ~27.5 GB reclaimed estimate",
        c: GREEN,
        b: true,
      },
    ],
    [],
    [{ t: "• trashed huggingface model cache — recoverable from Finder" }],
    [{ t: "• trashed npm cache — recoverable from Finder" }],
    [{ t: "• trashed Homebrew downloads — recoverable from Finder" }],
    [{ t: "• trashed uv cache — recoverable from Finder" }],
  ]);

const loadingBody = (): Row[] => {
  const msg = "Scanning caches…";
  const lines: Row[] = [];
  for (let i = 0; i < 7; i++) lines.push([]);
  lines.push([
    { t: " ".repeat(Math.floor((COLS - 2 - msg.length) / 2)) + msg, c: AMBER },
  ]);
  return boxed(" Working ", COLS, 17, lines);
};

const Popup: React.FC<{ caret: boolean }> = ({ caret }) => {
  const rows = boxed(" Confirm exact plan ", 96, 16, [
    [{ t: "DATA-LOSS WARNING", c: RED, b: true }],
    [],
    ...NOTICE.map((t): Row => [{ t }]),
    [],
    [
      {
        t: "Danger-3. Press y to apply this exact plan, or n/Esc to cancel.",
        c: AMBER,
      },
    ],
    [],
    [{ t: "> " }, { t: caret ? "█" : " ", c: GREEN, b: true }],
  ]);
  return (
    <div
      style={{
        position: "absolute",
        left: "2ch",
        top: 4 * LH,
        width: "96ch",
        background: BG,
      }}
    >
      <Screen rows={rows} />
    </div>
  );
};

const Keycap: React.FC<{ label: string }> = ({ label }) => (
  <div
    style={{
      position: "absolute",
      right: 90,
      bottom: 54,
      padding: "10px 22px",
      border: `1px solid ${LINE}`,
      borderRadius: 10,
      background: "#161616",
      color: GREEN,
      fontSize: 30,
      fontWeight: 700,
    }}
  >
    {label}
  </div>
);

// Timeline (30 fps).
const T = {
  moveSelection: 30,
  enter: 58,
  loading: 62,
  results: 88,
  applyKey: 196,
  confirm: 202,
  yesKey: 250,
  outcome: 258,
  end: 320,
};

export const DevtrimDemo: React.FC = () => {
  const frame = useCurrentFrame();

  const selected = frame >= T.moveSelection ? 1 : 0;

  let body: Row[];
  let keys: string;
  let status: string;
  let operation = "choose an operation";

  if (frame < T.loading) {
    body = menuBody(selected);
    keys = "↑/↓ navigate · Enter select · q quit";
    status = "Preview first. Nothing changes until you explicitly approve.";
  } else if (frame < T.results) {
    operation = "caches";
    body = loadingBody();
    keys = "Scanning and apply are synchronous; please wait";
    status = "Scanning caches…";
  } else if (frame < T.outcome) {
    operation = "caches";
    body = resultsBody();
    keys =
      frame >= T.confirm
        ? "Esc cancel · type the exact requested acknowledgment"
        : "a apply · s Trash/permanent · r rescan · b back · q quit";
    status =
      frame >= T.confirm
        ? "Confirm the exact previewed plan."
        : "Preview only. Apply moves these exact paths to Trash.";
  } else {
    operation = "caches";
    body = outcomeBody();
    keys = "↑/↓ or j/k scroll · b back to menu · q quit";
    status = "Applied the exact previewed plan. Recoverable from Trash.";
  }

  const keycap =
    frame >= T.enter && frame < T.loading
      ? "Enter"
      : frame >= T.applyKey && frame < T.confirm
        ? "a"
        : frame >= T.yesKey && frame < T.outcome
          ? "y"
          : null;

  const rows = [...header(operation), ...body, ...footer(keys, status)];

  const endOpacity = interpolate(frame, [T.end, T.end + 30], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  return (
    <AbsoluteFill
      style={{
        background: BG,
        alignItems: "center",
        justifyContent: "center",
        fontFamily: "Menlo, monospace",
        fontSize: FONT,
      }}
    >
      <div
        style={{
          border: `1px solid ${LINE}`,
          borderRadius: 14,
          background: "#0d0d0d",
          boxShadow: "0 24px 60px rgba(0,0,0,0.55)",
          overflow: "hidden",
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 8,
            padding: "10px 14px",
            borderBottom: `1px solid ${LINE}`,
            background: "#161616",
          }}
        >
          <span
            style={{
              width: 12,
              height: 12,
              borderRadius: "50%",
              background: "#ff5f57",
            }}
          />
          <span
            style={{
              width: 12,
              height: 12,
              borderRadius: "50%",
              background: "#febc2e",
            }}
          />
          <span
            style={{
              width: 12,
              height: 12,
              borderRadius: "50%",
              background: "#28c840",
            }}
          />
          <span style={{ marginLeft: 10, color: MUTED, fontSize: 18 }}>
            zsh — devtrim
          </span>
        </div>
        <div
          style={{
            position: "relative",
            padding: "14px 18px",
            lineHeight: `${LH}px`,
          }}
        >
          <Screen rows={rows} />
          {frame >= T.confirm && frame < T.outcome ? (
            <Popup caret={Math.floor(frame / 15) % 2 === 0} />
          ) : null}
        </div>
      </div>

      {keycap ? <Keycap label={keycap} /> : null}

      <AbsoluteFill
        style={{
          background: BG,
          opacity: endOpacity,
          alignItems: "center",
          justifyContent: "center",
          flexDirection: "column",
          gap: 20,
        }}
      >
        <div
          style={{
            fontSize: 120,
            fontWeight: 800,
            letterSpacing: "-0.04em",
            color: INK,
          }}
        >
          dev<span style={{ color: GREEN }}>trim</span>
        </div>
        <div style={{ fontSize: 30, color: MUTED }}>
          v0.6.2 · interactive Ratatui interface
        </div>
        <div style={{ fontSize: 26, color: DIM }}>
          github.com/mneves75/devtrim
        </div>
      </AbsoluteFill>
    </AbsoluteFill>
  );
};
