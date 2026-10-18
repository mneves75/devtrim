# Video tooling

Use the repository's existing npm/package-lock workflow here: `npm ci --strict-allow-scripts`.
`npm run lint` keeps Remotion's ESLint rules and checks the same three source files with
exact native TypeScript7.0.2 through `typescript-native`. `tsconfig.json` declares
ES2018 library types to match its existing ES2018 target and the source's use of
`String.padStart`. It keeps the existing exclusion of `remotion.config.ts`.

Classic TypeScript5.9.3 remains under `typescript` for Remotion's typed ESLint rules
and its SVGR/cosmiconfig TypeScript config loader. Remove that boundary only after
those consumers support the native API and valid/invalid lint and synchronous/
asynchronous config-loader controls pass. `npm run format:check` and `npm run build`
remain the other documented video gates; a source check does not render or publish a film.
