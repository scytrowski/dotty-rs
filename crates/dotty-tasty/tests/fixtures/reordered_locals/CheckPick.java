class CheckPick {
    public static void main(String[] args) {
        int result = new Locals().pick();
        if (result != 41) {
            throw new AssertionError("Expected pick() to return 41, got " + result);
        }
    }
}
