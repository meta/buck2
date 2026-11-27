# The ProGuard optimizations the Android SDK's last ProGuard-era `proguard-android-optimize.txt` kept
# (the Android Gradle Plugin's file has only `-allowaccessmodification`, because R8 ignores
# `-optimizations`). Class merging, for one, can give an interface a static initializer that D8 then
# desugars into an invalid non-final interface field for minSdk < 24.
-optimizations !code/simplification/arithmetic,!code/simplification/cast,!field/*,!class/merging/*
-optimizationpasses 5
