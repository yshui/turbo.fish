`::<> turbo.fish <>::`
======================

Feature rich, highly customizable, and natively asynchronous prompt for fish shell.

(Honestly I just thought this is a good name for a fish prompt.)

## Features

* **Obligatory: it's bLaZinGlY fAsT!!!11**
* **Customizable:** honestly too many configuration options.
* **Native Asynchronous:** prompt renders asynchronous in the background, so slow elements such as git does not block your shell. It's natively asynchronous so it's also more reliable, and doesn't suffer from the information loss problem async prompt wrappers have.
* **Loading Animation:** shows you a little spinner so you don't get bored as the prompt loads in the background.

## Installation

### Prerequisites

- A font patched with [Nerd Font](https://www.nerdfonts.com/) symbols. Here's [a list of pre-patched fonts](https://www.nerdfonts.com/font-downloads).

### Install turbo.fish

```bash
git clone https://github.com/yshui/turbo.fish
cd turbo.fish
cargo install --path .
```

### Setup turbo.fish

Add this to your `~/.config/fish/config.fish`:

```fish
turbofish init | source
```

## Acknowledgements

* [Starship](https://github.com/starship/starship) (copywriting, feature ideas)
* [bobthefish](https://github.com/oh-my-fish/theme-bobthefish) (design, feature ideas)
* [fish-async-prompt](https://github.com/acomagu/fish-async-prompt) (async implementation)
