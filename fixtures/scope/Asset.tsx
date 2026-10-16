type Props = { asset: string }

export const AssetEditor = (props: Props) => {
  const saveAssetNow = async () => {
    return props.asset
  }
  return saveAssetNow
}

export function BrandAssetPanel(props: Props) {
  const saveAssetNow = () => props.asset
  const rows = [saveAssetNow]
  return rows
}

export const standalone = 1
