import argparse


def parse_args():
    parser = argparse.ArgumentParser(description="count words in a file")
    parser.add_argument("path")
    parser.add_argument("--top", type=int, default=10)
    return parser.parse_args()


def main():
    args = parse_args()
    print(f"reading {args.path}")


if __name__ == "__main__":
    main()
