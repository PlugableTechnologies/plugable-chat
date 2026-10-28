interface SettingsSwitchProps {
    checked: boolean;
    onChange: (checked: boolean) => void;
    /** Spoken name and test handle, e.g. "Enable Sales BigQuery". */
    label: string;
    disabled?: boolean;
}

/** An on/off switch; the same look the MCP server cards use for "Enabled". */
export function SettingsSwitch({ checked, onChange, label, disabled = false }: SettingsSwitchProps) {
    return (
        <button
            type="button"
            role="switch"
            aria-checked={checked}
            aria-label={label}
            disabled={disabled}
            onClick={() => onChange(!checked)}
            className={`settings-switch relative w-10 h-5 shrink-0 rounded-full transition-colors disabled:opacity-50 ${checked ? 'bg-blue-500' : 'bg-gray-300'}`}
        >
            <span className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full shadow transition-transform ${checked ? 'translate-x-5' : ''}`} />
        </button>
    );
}
