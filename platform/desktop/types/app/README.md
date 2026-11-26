# {{name}}

A Lunarstorm desktop app: `src/app.tsx` declares the application, and each form is a
view (`src/forms/*.view.tsx`) paired by file name with its form class (`src/forms/*.ts`).

```text
lunar build -p . -o dist                 build into dist/
lunarstorm-debug dist                    run it, rebuilding on change
```
