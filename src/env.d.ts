// Ambient declarations for files the bundler handles. Kept local (instead of
// `vite/client`) so type-checking does not depend on the bundler's typings.

declare module '*.css';
declare module '*.woff' {
  const url: string;
  export default url;
}

interface ImportMetaEnv {
  readonly DEV: boolean;
  readonly PROD: boolean;
  readonly MODE: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
