import js from '@eslint/js'
import globals from 'globals'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'
import tseslint from 'typescript-eslint'

export default tseslint.config(
  { ignores: ['dist'] },
  {
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    files: ['**/*.{ts,tsx}'],
    languageOptions: {
      ecmaVersion: 2020,
      globals: globals.browser,
    },
    plugins: {
      'react-hooks': reactHooks,
      'react-refresh': reactRefresh,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // react-hooks 7系はReact Compiler向け規則もrecommendedへ追加したが、本プロジェクトは
      // React 18でCompiler未導入。従来から有効なhooks-of-rules/exhaustive-depsは維持し、
      // Compiler前提で既存の正当な非同期effect/refパターンを拒否する規則だけ無効化する。
      'react-hooks/immutability': 'off',
      'react-hooks/refs': 'off',
      'react-hooks/set-state-in-effect': 'off',
      'react-hooks/use-memo': 'off',
      // Context provider と利用hookを同じモジュールから公開する設計のため、
      // コンポーネント専用exportを前提とするFast Refresh規則は適用しない。
      'react-refresh/only-export-components': 'off',
    },
  },
)
