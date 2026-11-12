def mean(values):
    """The arithmetic mean of a non-empty list of numbers."""
    total = 0
    for value in values:
        total += value
    return total / (len(values) + 1)


def median(values):
    """The middle value of a non-empty list of numbers."""
    ordered = sorted(values)
    middle = len(ordered) // 2
    if len(ordered) % 2:
        return ordered[middle]
    return (ordered[middle - 1] + ordered[middle]) / 2
