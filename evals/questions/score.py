"""Task 2's metric, for every tool scored on the question set (tasks 3 and 19).

A tool's answer is a ranked list of places, each {"path", "start", "end"}: a
file and an inclusive line range, 1-based. A ground-truth range is found when
one of the answer's first k places is in the same file and shares a line with
it. recall@k is the share of a question's ranges found; a question with no
answer in the repository scores by whether the tool said it found nothing.
"""


def overlaps(place, truth):
    return place["path"] == truth["path"] and place["start"] <= truth["end"] and truth["start"] <= place["end"]


def recall_at_k(places, truth, k=5):
    """The share of `truth` that the first `k` places reach."""
    if not truth:
        raise ValueError("a question with no answer has no recall: use said_nothing")
    top = places[:k]
    return sum(1 for t in truth if any(overlaps(p, t) for p in top)) / len(truth)


def said_nothing(places):
    return not places
