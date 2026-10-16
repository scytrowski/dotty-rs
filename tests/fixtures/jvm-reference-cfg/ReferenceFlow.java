final class Counter {
    private int value;

    Counter(int value) {
        this.value = value;
    }

    int increment() {
        return ++value;
    }

    int get() {
        return value;
    }
}

public final class ReferenceFlow {
    static int loop(Counter counter, int remaining) {
        while (remaining > 0) {
            counter.increment();
            remaining--;
        }
        return counter.get();
    }

    int branchOnThis(boolean increment) {
        if (increment) {
            return loop(new Counter(get()), 1);
        }
        return 0;
    }

    static Counter sameClassJoin(boolean left) {
        Counter selected;
        if (left) {
            selected = new Counter(3);
        } else {
            selected = new Counter(4);
        }
        return selected;
    }

    static Counter newInBranch(boolean left) {
        if (left) {
            return new Counter(5);
        }
        return new Counter(6);
    }
}
