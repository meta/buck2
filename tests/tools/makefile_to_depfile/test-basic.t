  $ $DEP_FILE_PROCESSOR "makefile" "$TESTDIR/fixtures/basic.mk" "$CRAMTMP/basic" true
  $ cat "$CRAMTMP/basic"
  app.c
  header.h

  $ printf "app: app.c header.h" > "$CRAMTMP/no-trailing-newline.mk"
  $ $DEP_FILE_PROCESSOR "makefile" "$CRAMTMP/no-trailing-newline.mk" "$CRAMTMP/no-trailing-newline" true
  $ cat "$CRAMTMP/no-trailing-newline"
  app.c
  header.h
