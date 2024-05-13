#!/bin/bash

set -euo pipefail

source .env/bin/activate
maturin develop --release

exit 0
