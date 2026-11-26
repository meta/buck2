For fast js dev iteration do this (tested on Mac):

```
# run a build command, then refresh the generated schema
cp -rfX "$(buck2 build //buck2/app/buck2_explain/output_format_js:schema_ts --show-full-simple-output)" src
yarn watch
```

open index.html in the browser
