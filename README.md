# Kronon Code

<p align="center">
  <a href="https://github.com/lpjhelder/Kronon-Code">lpjhelder/Kronon-Code</a>
  ·
  <a href="./USAGE.md">Usage</a>
  ·
  <a href="./rust/README.md">Rust workspace</a>
  ·
  <a href="./CHANGELOG.md">Changelog</a>
</p>

Originado do projeto claw-code, agora independente como Kronon Code. Foco em **suporte a Windows** e **modelos locais via Ollama**.

## O que muda

- **Suporte a Windows** — bash tool usa `cmd.exe /C` no Windows em vez de `sh -lc`
- **Ollama como backend** — configurado pra usar modelos locais via API OpenAI-compatible
- **Testado com Gemma 4** — validado com gemma4:26b e gemma4:31b em RTX PRO 5000 Blackwell (48GB VRAM)

## Quick start

### Pre-requisitos

- [Rust toolchain](https://rustup.rs/) (cargo)
- [Ollama](https://ollama.com/) rodando com um modelo (ex: `gemma4:31b`)

### Build

```bash
git clone https://github.com/lpjhelder/Kronon-Code.git
cd Kronon-Code/rust
cargo build --workspace
```

### Configurar (uma vez)

Windows (CMD):
```cmd
setx OPENAI_BASE_URL "http://192.168.0.10:11434/v1"
setx OPENAI_API_KEY "dummy"
```

Linux/Mac:
```bash
export OPENAI_BASE_URL="http://localhost:11434/v1"
export OPENAI_API_KEY="dummy"
```

> Ajuste o IP/porta pro seu servidor Ollama. O `OPENAI_API_KEY` pode ser qualquer valor — Ollama nao exige autenticacao.

### Usar

```bash
# REPL interativo
./rust/target/debug/kronon --model gemma4:31b

# Prompt unico
./rust/target/debug/kronon --model gemma4:31b prompt "explica o que esse projeto faz"
```

## Modelos testados

| Modelo | VRAM | tok/s | Tool calling | Status |
|--------|:----:|:-----:|:------------:|:------:|
| gemma4:31b (denso, Q4) | 47.3 GB | 48 tok/s | Funciona | GPU 100% (sem TTS) |
| gemma4:26b (MoE, Q4) | 25.9 GB | 135 tok/s | A testar | GPU 100% |

> Testado em NVIDIA RTX PRO 5000 Blackwell 48GB via Ollama 0.20.4

## Estrutura do projeto

- **`rust/`** — workspace Rust com o binario `kronon`
- **`USAGE.md`** — guia de uso (comandos, auth, sessoes)
- **`PARITY.md`** — status de paridade com o projeto original
- **`ROADMAP.md`** — roadmap e backlog

## Origem

Este projeto foi originalmente baseado no claw-code (ultraworkers/claw-code) mas agora e desenvolvido de forma independente como Kronon Code. Nao ha mais sincronizacao ativa com o upstream.

## Disclaimer

- Este repositorio **nao** reivindica propriedade do codigo fonte original do Claude Code.
- Este repositorio **nao e afiliado, endossado ou mantido pela Anthropic**.
- Fork mantido por [@lpjhelder](https://github.com/lpjhelder).
