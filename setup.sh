#!/bin/bash

set -euo pipefail

python3 -m venv .env
source .env/bin/activate
pip install maturin
pip install patchelf

exit 0
