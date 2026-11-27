# Rules for ProGuard as Buck runs it, added to the Android default configs.

# Buck runs ProGuard on class files, before D8 desugars them, so invokedynamic lambdas and string
# concatenation still reference these JDK bootstrap classes, which android.jar does not have.
-dontwarn java.lang.invoke.LambdaMetafactory
-dontwarn java.lang.invoke.StringConcatFactory
