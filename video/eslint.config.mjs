import { config } from "@remotion/eslint-config-flat";

export default [
  ...config,
  {
    // Every on-screen string comes from film/copy.json through line(); check_facts.py traces it to facts.json.
    files: ["src/DevtrimDemo.tsx", "src/film/**/*.tsx"],
    rules: {
      "react/jsx-no-literals": ["error", { noStrings: true, ignoreProps: true }],
    },
  },
];
