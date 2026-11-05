# Test case 1: tuples with different lengths and types
# (1,) can compare with (1, 'a') up to first element
# (1,) can compare with (1, 2) up to first element
# But can (1, 'a') compare with (1, 2)?
print("Test 1: Tuples with mixed types")
try:
    result = sorted([(1,), (1, 'a'), (1, 2)])
    print("  SUCCESS: sorted result =", result)
except Exception as e:
    print("  ERROR:", e)

# Test case 2: None with other types
print("\nTest 2: None mixed with other types")
try:
    result = sorted([None, 1, "hello"])
    print("  SUCCESS: sorted result =", result)
except Exception as e:
    print("  ERROR:", e)

# Test case 3: Different numeric types
print("\nTest 3: int, float, bool mixed")
try:
    result = sorted([5, 3.14, True, 0, False, 2.71])
    print("  SUCCESS: sorted result =", result)
except Exception as e:
    print("  ERROR:", e)

# Test case 4: Lists with mixed element types
print("\nTest 4: Lists with mixed element types")
try:
    result = sorted([[1], [1, 'a'], [1, 2]])
    print("  SUCCESS: sorted result =", result)
except Exception as e:
    print("  ERROR:", e)

print("\nAll tests complete")
