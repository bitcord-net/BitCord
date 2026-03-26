import js from "@eslint/js";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import tseslint from "@typescript-eslint/eslint-plugin";
import tsParser from "@typescript-eslint/parser";

export default [
  { ignores: ["dist", "node_modules"] },
  // Node config files
  {
    files: ["*.config.ts", "*.config.js", "scripts/**"],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      parser: tsParser,
      globals: { ...globals.node },
    },
    plugins: {
      "@typescript-eslint": tseslint,
    },
    rules: {
      ...js.configs.recommended.rules,
      ...tseslint.configs.recommended.rules,
    },
  },
  // App source
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      parser: tsParser,
      parserOptions: {
        ecmaFeatures: { jsx: true },
      },
      globals: globals.browser,
    },
    plugins: {
      "@typescript-eslint": tseslint,
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      ...js.configs.recommended.rules,
      ...tseslint.configs.recommended.rules,
      ...reactHooks.configs.recommended.rules,
      "@typescript-eslint/no-unused-vars": [
        "error",
        { ignoreRestSiblings: true },
      ],
      "react-refresh/only-export-components": [
        "warn",
        { allowConstantExport: true },
      ],
    },
  },
];
