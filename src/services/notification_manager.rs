// SPDX-License-Identifier: GPL-3.0-or-later

use crate::args::MosaicArgs;
use crate::config::SessionDefaults;
pub fn start(_args: &MosaicArgs, _session: &SessionDefaults) -> anyhow::Result<()> {
    Ok(())
}
pub fn stop(_args: &MosaicArgs) -> anyhow::Result<()> {
    Ok(())
}
