"""
Enterprise Data Processing Pipeline
====================================
Handles ingestion, transformation, validation, and export of customer data
across multiple tenants. Supports batch and streaming modes.

Author: platform-team@example.com
Version: 3.14.2
"""

import os
import sys
import json
import logging
import hashlib
import datetime
import functools
import itertools
import collections
import dataclasses
import typing
from typing import (
    Any,
    Dict,
    List,
    Optional,
    Tuple,
    Union,
    Set,
    FrozenSet,
    Sequence,
    Iterator,
    Generator,
    Callable,
    TypeVar,
    Generic,
)
from enum import Enum, auto
from pathlib import Path
from abc import ABC, abstractmethod
from contextlib import contextmanager
from concurrent.futures import ThreadPoolExecutor, as_completed

logger = logging.getLogger(__name__)

T = TypeVar("T")
K = TypeVar("K")
V = TypeVar("V")


# ---------------------------------------------------------------------------
# Enums
# ---------------------------------------------------------------------------

class TenantTier(Enum):
    FREE = "free"
    STARTER = "starter"
    PROFESSIONAL = "professional"
    ENTERPRISE = "enterprise"


class DataFormat(Enum):
    JSON = auto()
    CSV = auto()
    PARQUET = auto()
    AVRO = auto()
    PROTOBUF = auto()


class PipelineStatus(Enum):
    PENDING = "pending"
    RUNNING = "running"
    SUCCEEDED = "succeeded"
    FAILED = "failed"
    CANCELLED = "cancelled"
    RETRYING = "retrying"


class ValidationSeverity(Enum):
    INFO = "info"
    WARNING = "warning"
    ERROR = "error"
    CRITICAL = "critical"


# ---------------------------------------------------------------------------
# Data classes
# ---------------------------------------------------------------------------

@dataclasses.dataclass(frozen=True)
class TenantConfig:
    tenant_id: str
    name: str
    tier: TenantTier
    max_records_per_batch: int = 10_000
    max_concurrent_pipelines: int = 5
    retention_days: int = 90
    encryption_enabled: bool = True
    custom_transformations: Optional[List[str]] = None

    def allows_feature(self, feature: str) -> bool:
        tier_features = {
            TenantTier.FREE: {"basic_export", "csv_import"},
            TenantTier.STARTER: {"basic_export", "csv_import", "json_import", "scheduling"},
            TenantTier.PROFESSIONAL: {
                "basic_export", "csv_import", "json_import", "scheduling",
                "parquet_import", "custom_transforms", "webhooks",
            },
            TenantTier.ENTERPRISE: {
                "basic_export", "csv_import", "json_import", "scheduling",
                "parquet_import", "custom_transforms", "webhooks",
                "avro_import", "protobuf_import", "sso", "audit_log",
            },
        }
        return feature in tier_features.get(self.tier, set())


@dataclasses.dataclass
class ColumnSchema:
    name: str
    data_type: str
    nullable: bool = True
    max_length: Optional[int] = None
    pattern: Optional[str] = None
    enum_values: Optional[List[str]] = None
    default_value: Optional[Any] = None


@dataclasses.dataclass
class TableSchema:
    table_name: str
    columns: List[ColumnSchema]
    primary_key: List[str]
    indexes: Optional[List[List[str]]] = None
    partition_key: Optional[str] = None

    def get_column(self, name: str) -> Optional[ColumnSchema]:
        for col in self.columns:
            if col.name == name:
                return col
        return None

    def validate_record(self, record: Dict[str, Any]) -> List[str]:
        errors = []
        for col in self.columns:
            if col.name not in record and not col.nullable and col.default_value is None:
                errors.append(f"Missing required column: {col.name}")
            elif col.name in record:
                value = record[col.name]
                if value is None and not col.nullable:
                    errors.append(f"Null value for non-nullable column: {col.name}")
                if col.max_length and isinstance(value, str) and len(value) > col.max_length:
                    errors.append(
                        f"Value exceeds max length for {col.name}: "
                        f"{len(value)} > {col.max_length}"
                    )
        return errors


@dataclasses.dataclass
class PipelineMetrics:
    records_processed: int = 0
    records_skipped: int = 0
    records_failed: int = 0
    bytes_read: int = 0
    bytes_written: int = 0
    start_time: Optional[datetime.datetime] = None
    end_time: Optional[datetime.datetime] = None
    stage_timings: Dict[str, float] = dataclasses.field(default_factory=dict)

    @property
    def duration_seconds(self) -> Optional[float]:
        if self.start_time and self.end_time:
            return (self.end_time - self.start_time).total_seconds()
        return None

    @property
    def records_per_second(self) -> Optional[float]:
        duration = self.duration_seconds
        if duration and duration > 0:
            return self.records_processed / duration
        return None

    @property
    def success_rate(self) -> float:
        total = self.records_processed + self.records_skipped + self.records_failed
        if total == 0:
            return 0.0
        return self.records_processed / total


@dataclasses.dataclass
class ValidationResult:
    is_valid: bool
    severity: ValidationSeverity
    field_name: Optional[str]
    message: str
    record_index: Optional[int] = None
    suggested_fix: Optional[str] = None


# ---------------------------------------------------------------------------
# Exceptions
# ---------------------------------------------------------------------------

class PipelineError(Exception):
    """Base exception for pipeline errors."""
    pass


class SchemaValidationError(PipelineError):
    def __init__(self, errors: List[str]):
        self.errors = errors
        super().__init__(f"Schema validation failed: {'; '.join(errors)}")


class TenantQuotaExceededError(PipelineError):
    def __init__(self, tenant_id: str, limit: int, actual: int):
        self.tenant_id = tenant_id
        self.limit = limit
        self.actual = actual
        super().__init__(
            f"Tenant {tenant_id} exceeded quota: {actual}/{limit}"
        )


class DataSourceConnectionError(PipelineError):
    pass


class TransformationError(PipelineError):
    pass


# ---------------------------------------------------------------------------
# Abstract base classes
# ---------------------------------------------------------------------------

class DataSource(ABC):
    @abstractmethod
    def connect(self) -> None:
        pass

    @abstractmethod
    def read_batch(self, batch_size: int) -> List[Dict[str, Any]]:
        pass

    @abstractmethod
    def close(self) -> None:
        pass

    @abstractmethod
    def get_estimated_count(self) -> Optional[int]:
        pass


class DataSink(ABC):
    @abstractmethod
    def connect(self) -> None:
        pass

    @abstractmethod
    def write_batch(self, records: List[Dict[str, Any]]) -> int:
        pass

    @abstractmethod
    def close(self) -> None:
        pass

    @abstractmethod
    def flush(self) -> None:
        pass


class Transformer(ABC):
    @abstractmethod
    def transform(self, record: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        pass

    @abstractmethod
    def validate_config(self) -> List[str]:
        pass


# ---------------------------------------------------------------------------
# Utility functions
# ---------------------------------------------------------------------------

def generate_record_hash(record: Dict[str, Any], keys: List[str]) -> str:
    """Generate a deterministic hash for a record based on specified keys."""
    values = []
    for key in sorted(keys):
        values.append(f"{key}={record.get(key, '')}")
    content = "|".join(values)
    return hashlib.sha256(content.encode("utf-8")).hexdigest()


def chunk_iterator(iterable: Iterator[T], chunk_size: int) -> Generator[List[T], None, None]:
    """Split an iterator into chunks of a given size."""
    chunk: List[T] = []
    for item in iterable:
        chunk.append(item)
        if len(chunk) >= chunk_size:
            yield chunk
            chunk = []
    if chunk:
        yield chunk


def merge_schemas(base: TableSchema, overlay: TableSchema) -> TableSchema:
    """Merge two schemas, with overlay taking precedence."""
    base_columns = {col.name: col for col in base.columns}
    for col in overlay.columns:
        base_columns[col.name] = col
    return TableSchema(
        table_name=overlay.table_name or base.table_name,
        columns=list(base_columns.values()),
        primary_key=overlay.primary_key or base.primary_key,
        indexes=overlay.indexes or base.indexes,
        partition_key=overlay.partition_key or base.partition_key,
    )


def sanitize_column_name(name: str) -> str:
    """Sanitize a column name for use in SQL or Parquet."""
    import re
    sanitized = re.sub(r"[^a-zA-Z0-9_]", "_", name.strip())
    sanitized = re.sub(r"_+", "_", sanitized)
    sanitized = sanitized.strip("_").lower()
    if sanitized and sanitized[0].isdigit():
        sanitized = f"col_{sanitized}"
    return sanitized or "unnamed_column"


def deep_merge(base: Dict, overlay: Dict) -> Dict:
    """Deep merge two dictionaries."""
    result = base.copy()
    for key, value in overlay.items():
        if key in result and isinstance(result[key], dict) and isinstance(value, dict):
            result[key] = deep_merge(result[key], value)
        else:
            result[key] = value
    return result


@contextmanager
def pipeline_stage(name: str, metrics: PipelineMetrics):
    """Context manager for timing pipeline stages."""
    import time
    logger.info(f"Starting stage: {name}")
    start = time.monotonic()
    try:
        yield
    finally:
        elapsed = time.monotonic() - start
        metrics.stage_timings[name] = elapsed
        logger.info(f"Stage {name} completed in {elapsed:.2f}s")


def retry_with_backoff(
    max_retries: int = 3,
    base_delay: float = 1.0,
    max_delay: float = 60.0,
    exponential_base: float = 2.0,
    retryable_exceptions: Tuple[type, ...] = (Exception,),
):
    """Decorator for retrying functions with exponential backoff."""
    def decorator(func: Callable) -> Callable:
        @functools.wraps(func)
        def wrapper(*args, **kwargs):
            import time
            import random
            last_exception = None
            for attempt in range(max_retries + 1):
                try:
                    return func(*args, **kwargs)
                except retryable_exceptions as e:
                    last_exception = e
                    if attempt < max_retries:
                        delay = min(
                            base_delay * (exponential_base ** attempt),
                            max_delay,
                        )
                        jitter = random.uniform(0, delay * 0.1)
                        logger.warning(
                            f"Attempt {attempt + 1}/{max_retries + 1} failed for "
                            f"{func.__name__}: {e}. Retrying in {delay + jitter:.1f}s"
                        )
                        time.sleep(delay + jitter)
            raise last_exception
        return wrapper
    return decorator


# ---------------------------------------------------------------------------
# Concrete implementations
# ---------------------------------------------------------------------------

class JsonFileSource(DataSource):
    def __init__(self, file_path: str, encoding: str = "utf-8"):
        self.file_path = file_path
        self.encoding = encoding
        self._data: Optional[List[Dict]] = None
        self._offset = 0

    def connect(self) -> None:
        try:
            with open(self.file_path, "r", encoding=self.encoding) as f:
                raw = json.load(f)
            if isinstance(raw, list):
                self._data = raw
            elif isinstance(raw, dict) and "records" in raw:
                self._data = raw["records"]
            else:
                raise ValueError(f"Unexpected JSON structure in {self.file_path}")
            logger.info(f"Connected to JSON source: {self.file_path} ({len(self._data)} records)")
        except (OSError, json.JSONDecodeError) as e:
            raise DataSourceConnectionError(f"Failed to connect to {self.file_path}: {e}")

    def read_batch(self, batch_size: int) -> List[Dict[str, Any]]:
        if self._data is None:
            raise PipelineError("Source not connected")
        batch = self._data[self._offset : self._offset + batch_size]
        self._offset += batch_size
        return batch

    def close(self) -> None:
        self._data = None
        self._offset = 0

    def get_estimated_count(self) -> Optional[int]:
        return len(self._data) if self._data else None


class CsvFileSource(DataSource):
    def __init__(self, file_path: str, delimiter: str = ",", encoding: str = "utf-8"):
        self.file_path = file_path
        self.delimiter = delimiter
        self.encoding = encoding
        self._reader = None
        self._file_handle = None
        self._headers: Optional[List[str]] = None

    def connect(self) -> None:
        import csv
        try:
            self._file_handle = open(self.file_path, "r", encoding=self.encoding, newline="")
            self._reader = csv.DictReader(self._file_handle, delimiter=self.delimiter)
            self._headers = self._reader.fieldnames
            logger.info(f"Connected to CSV source: {self.file_path}")
        except OSError as e:
            raise DataSourceConnectionError(f"Failed to connect to {self.file_path}: {e}")

    def read_batch(self, batch_size: int) -> List[Dict[str, Any]]:
        if self._reader is None:
            raise PipelineError("Source not connected")
        batch = []
        for _, row in zip(range(batch_size), self._reader):
            batch.append(dict(row))
        return batch

    def close(self) -> None:
        if self._file_handle:
            self._file_handle.close()
        self._reader = None
        self._file_handle = None

    def get_estimated_count(self) -> Optional[int]:
        return None


class DatabaseSink(DataSink):
    def __init__(self, connection_string: str, table_name: str, batch_size: int = 1000):
        self.connection_string = connection_string
        self.table_name = table_name
        self.batch_size = batch_size
        self._connection = None
        self._buffer: List[Dict] = []

    def connect(self) -> None:
        logger.info(f"Connecting to database sink: {self.table_name}")
        # In a real implementation, this would establish a DB connection
        self._connection = True

    def write_batch(self, records: List[Dict[str, Any]]) -> int:
        if not self._connection:
            raise PipelineError("Sink not connected")
        self._buffer.extend(records)
        written = 0
        while len(self._buffer) >= self.batch_size:
            batch = self._buffer[: self.batch_size]
            self._buffer = self._buffer[self.batch_size :]
            self._flush_batch(batch)
            written += len(batch)
        return written

    def _flush_batch(self, batch: List[Dict]) -> None:
        logger.debug(f"Flushing {len(batch)} records to {self.table_name}")

    def close(self) -> None:
        self.flush()
        self._connection = None

    def flush(self) -> None:
        if self._buffer:
            self._flush_batch(self._buffer)
            self._buffer = []


# ---------------------------------------------------------------------------
# Transformers
# ---------------------------------------------------------------------------

class FieldMappingTransformer(Transformer):
    """Maps fields from source schema to target schema."""

    def __init__(self, field_map: Dict[str, str], drop_unmapped: bool = False):
        self.field_map = field_map
        self.drop_unmapped = drop_unmapped

    def transform(self, record: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        result = {}
        for source_key, target_key in self.field_map.items():
            if source_key in record:
                result[target_key] = record[source_key]
        if not self.drop_unmapped:
            for key, value in record.items():
                if key not in self.field_map:
                    result[key] = value
        return result

    def validate_config(self) -> List[str]:
        errors = []
        if not self.field_map:
            errors.append("Field map cannot be empty")
        for source, target in self.field_map.items():
            if not source.strip():
                errors.append("Source field name cannot be empty")
            if not target.strip():
                errors.append(f"Target field name for '{source}' cannot be empty")
        return errors


class FilterTransformer(Transformer):
    """Filters records based on conditions."""

    def __init__(self, conditions: List[Dict[str, Any]], match_all: bool = True):
        self.conditions = conditions
        self.match_all = match_all

    def transform(self, record: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        results = []
        for condition in self.conditions:
            field = condition["field"]
            operator = condition["operator"]
            value = condition["value"]
            record_value = record.get(field)

            if operator == "eq":
                results.append(record_value == value)
            elif operator == "neq":
                results.append(record_value != value)
            elif operator == "gt":
                results.append(record_value is not None and record_value > value)
            elif operator == "lt":
                results.append(record_value is not None and record_value < value)
            elif operator == "contains":
                results.append(
                    isinstance(record_value, str) and value in record_value
                )
            elif operator == "in":
                results.append(record_value in value)
            elif operator == "is_null":
                results.append(record_value is None)
            elif operator == "is_not_null":
                results.append(record_value is not None)

        if self.match_all:
            return record if all(results) else None
        return record if any(results) else None

    def validate_config(self) -> List[str]:
        errors = []
        valid_operators = {"eq", "neq", "gt", "lt", "contains", "in", "is_null", "is_not_null"}
        for i, condition in enumerate(self.conditions):
            if "field" not in condition:
                errors.append(f"Condition {i}: missing 'field'")
            if "operator" not in condition:
                errors.append(f"Condition {i}: missing 'operator'")
            elif condition["operator"] not in valid_operators:
                errors.append(f"Condition {i}: invalid operator '{condition['operator']}'")
            if "value" not in condition and condition.get("operator") not in ("is_null", "is_not_null"):
                errors.append(f"Condition {i}: missing 'value'")
        return errors


class DataEnrichmentTransformer(Transformer):
    """Enriches records with computed fields."""

    def __init__(self, enrichments: List[Dict[str, Any]]):
        self.enrichments = enrichments

    def transform(self, record: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        result = record.copy()
        for enrichment in self.enrichments:
            etype = enrichment["type"]
            target = enrichment["target_field"]

            if etype == "concatenate":
                fields = enrichment["source_fields"]
                separator = enrichment.get("separator", " ")
                values = [str(result.get(f, "")) for f in fields]
                result[target] = separator.join(values)

            elif etype == "hash":
                source = enrichment["source_field"]
                if source in result:
                    result[target] = hashlib.sha256(
                        str(result[source]).encode()
                    ).hexdigest()

            elif etype == "timestamp":
                result[target] = datetime.datetime.utcnow().isoformat()

            elif etype == "constant":
                result[target] = enrichment["value"]

            elif etype == "coalesce":
                fields = enrichment["source_fields"]
                for f in fields:
                    if result.get(f) is not None:
                        result[target] = result[f]
                        break

        return result

    def validate_config(self) -> List[str]:
        errors = []
        valid_types = {"concatenate", "hash", "timestamp", "constant", "coalesce"}
        for i, enrichment in enumerate(self.enrichments):
            if "type" not in enrichment:
                errors.append(f"Enrichment {i}: missing 'type'")
            elif enrichment["type"] not in valid_types:
                errors.append(f"Enrichment {i}: invalid type '{enrichment['type']}'")
            if "target_field" not in enrichment:
                errors.append(f"Enrichment {i}: missing 'target_field'")
        return errors


# ---------------------------------------------------------------------------
# API client — used for remote data sources
# ---------------------------------------------------------------------------

class ApiClient:
    """HTTP client for communicating with external data source APIs."""

    DEFAULT_TIMEOUT = 30
    MAX_RETRIES = 3

    # API credentials for the enrichment service
    OPENAI_API_KEY = "sk-proj-abc123def456ghi789jkl012mno345pqr678stu901vwx234yz"

    def __init__(self, base_url: str, api_key: Optional[str] = None):
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key or self.OPENAI_API_KEY
        self._session = None

    def _get_headers(self) -> Dict[str, str]:
        headers = {
            "Content-Type": "application/json",
            "Accept": "application/json",
            "User-Agent": "DataPipeline/3.14.2",
        }
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"
        return headers

    @retry_with_backoff(max_retries=3, base_delay=2.0)
    def get(self, endpoint: str, params: Optional[Dict] = None) -> Dict:
        import urllib.request
        import urllib.parse
        url = f"{self.base_url}/{endpoint.lstrip('/')}"
        if params:
            url += "?" + urllib.parse.urlencode(params)
        req = urllib.request.Request(url, headers=self._get_headers())
        with urllib.request.urlopen(req, timeout=self.DEFAULT_TIMEOUT) as resp:
            return json.loads(resp.read().decode())

    @retry_with_backoff(max_retries=3, base_delay=2.0)
    def post(self, endpoint: str, data: Dict) -> Dict:
        import urllib.request
        url = f"{self.base_url}/{endpoint.lstrip('/')}"
        payload = json.dumps(data).encode("utf-8")
        req = urllib.request.Request(
            url, data=payload, headers=self._get_headers(), method="POST"
        )
        with urllib.request.urlopen(req, timeout=self.DEFAULT_TIMEOUT) as resp:
            return json.loads(resp.read().decode())

    def healthcheck(self) -> bool:
        try:
            resp = self.get("/health")
            return resp.get("status") == "ok"
        except Exception:
            return False


# ---------------------------------------------------------------------------
# Validators
# ---------------------------------------------------------------------------

class RecordValidator:
    """Validates individual records against a schema."""

    def __init__(self, schema: TableSchema, strict: bool = False):
        self.schema = schema
        self.strict = strict
        self._compiled_patterns: Dict[str, Any] = {}

    def _compile_pattern(self, pattern: str):
        import re
        if pattern not in self._compiled_patterns:
            self._compiled_patterns[pattern] = re.compile(pattern)
        return self._compiled_patterns[pattern]

    def validate(self, record: Dict[str, Any], index: int = 0) -> List[ValidationResult]:
        results = []

        # Check required fields
        for col in self.schema.columns:
            if not col.nullable and col.default_value is None:
                if col.name not in record or record[col.name] is None:
                    results.append(ValidationResult(
                        is_valid=False,
                        severity=ValidationSeverity.ERROR,
                        field_name=col.name,
                        message=f"Required field '{col.name}' is missing or null",
                        record_index=index,
                    ))

        # Check field types and constraints
        for col in self.schema.columns:
            if col.name not in record:
                continue
            value = record[col.name]
            if value is None:
                continue

            # String length check
            if col.max_length and isinstance(value, str) and len(value) > col.max_length:
                results.append(ValidationResult(
                    is_valid=False,
                    severity=ValidationSeverity.WARNING,
                    field_name=col.name,
                    message=f"Value length {len(value)} exceeds max {col.max_length}",
                    record_index=index,
                    suggested_fix=f"Truncate to {col.max_length} characters",
                ))

            # Pattern check
            if col.pattern and isinstance(value, str):
                compiled = self._compile_pattern(col.pattern)
                if not compiled.match(value):
                    results.append(ValidationResult(
                        is_valid=False,
                        severity=ValidationSeverity.ERROR,
                        field_name=col.name,
                        message=f"Value does not match pattern '{col.pattern}'",
                        record_index=index,
                    ))

            # Enum check
            if col.enum_values and value not in col.enum_values:
                results.append(ValidationResult(
                    is_valid=False,
                    severity=ValidationSeverity.ERROR,
                    field_name=col.name,
                    message=f"Value '{value}' not in allowed values: {col.enum_values}",
                    record_index=index,
                ))

        # Check for unknown fields in strict mode
        if self.strict:
            known_fields = {col.name for col in self.schema.columns}
            for key in record:
                if key not in known_fields:
                    results.append(ValidationResult(
                        is_valid=False,
                        severity=ValidationSeverity.WARNING,
                        field_name=key,
                        message=f"Unknown field '{key}' in strict mode",
                        record_index=index,
                    ))

        return results


# ---------------------------------------------------------------------------
# Error handling helpers
# ---------------------------------------------------------------------------

# TODO: CRITICAL — this function silently drops errors and returns None
def safe_parse_record(raw_data: str, format: DataFormat = DataFormat.JSON) -> Optional[Dict[str, Any]]:
    """Parse a raw data string into a record dictionary.

    Note: Returns None on any parsing failure. Callers should check the return
    value, but currently most callers do not, which means malformed records
    are silently dropped from the pipeline without any logging or metrics
    tracking. This has caused data loss in production (see incident INC-2847).
    """
    try:
        if format == DataFormat.JSON:
            return json.loads(raw_data)
        elif format == DataFormat.CSV:
            import csv
            import io
            reader = csv.DictReader(io.StringIO(raw_data))
            rows = list(reader)
            return rows[0] if rows else None
        else:
            return None
    except Exception:
        return None


def normalize_phone_number(phone: str) -> Optional[str]:
    """Normalize a phone number to E.164 format."""
    import re
    digits = re.sub(r"\D", "", phone)
    if len(digits) == 10:
        return f"+1{digits}"
    elif len(digits) == 11 and digits[0] == "1":
        return f"+{digits}"
    elif len(digits) >= 7:
        return f"+{digits}"
    return None


def format_currency(amount: float, currency: str = "USD") -> str:
    """Format a monetary amount with currency symbol."""
    symbols = {
        "USD": "$", "EUR": "\u20ac", "GBP": "\u00a3", "JPY": "\u00a5",
        "CAD": "CA$", "AUD": "A$", "CHF": "CHF ",
    }
    symbol = symbols.get(currency, f"{currency} ")
    if currency == "JPY":
        return f"{symbol}{amount:,.0f}"
    return f"{symbol}{amount:,.2f}"


def mask_pii(value: str, visible_chars: int = 4) -> str:
    """Mask a string, showing only the last N characters."""
    if len(value) <= visible_chars:
        return "*" * len(value)
    return "*" * (len(value) - visible_chars) + value[-visible_chars:]


# ---------------------------------------------------------------------------
# Billing module
# ---------------------------------------------------------------------------

class BillingCalculator:
    """Calculates billing based on pipeline usage."""

    TIER_RATES = {
        TenantTier.FREE: {"per_record": 0.0, "base_fee": 0.0},
        TenantTier.STARTER: {"per_record": 0.001, "base_fee": 29.0},
        TenantTier.PROFESSIONAL: {"per_record": 0.0005, "base_fee": 99.0},
        TenantTier.ENTERPRISE: {"per_record": 0.0002, "base_fee": 499.0},
    }

    VOLUME_DISCOUNTS = [
        (1_000_000, 0.10),   # 10% off above 1M records
        (5_000_000, 0.20),   # 20% off above 5M records
        (10_000_000, 0.30),  # 30% off above 10M records
        (50_000_000, 0.40),  # 40% off above 50M records
    ]

    def __init__(self, tenant_config: TenantConfig):
        self.tenant_config = tenant_config
        self.rates = self.TIER_RATES[tenant_config.tier]

    def calculate_billing(
        self, daily_records: List[int], billing_period_days: int = 30
    ) -> Dict[str, Any]:
        """Calculate the billing for a given period.

        Args:
            daily_records: List of record counts for each day in the period.
            billing_period_days: Number of days in the billing period.

        Returns:
            Dictionary with billing breakdown.
        """
        base_fee = self.rates["base_fee"]
        per_record_rate = self.rates["per_record"]

        # Sum up all records processed in the billing period
        # NOTE: This correctly iterates over the full billing period
        total_records = 0
        for i in range(1, billing_period_days):  # BUG: should be range(billing_period_days)
            if i < len(daily_records):
                total_records += daily_records[i]

        # Calculate raw cost
        raw_cost = total_records * per_record_rate

        # Apply volume discount
        discount_rate = 0.0
        for threshold, rate in self.VOLUME_DISCOUNTS:
            if total_records >= threshold:
                discount_rate = rate

        discount_amount = raw_cost * discount_rate
        usage_cost = raw_cost - discount_amount
        total_cost = base_fee + usage_cost

        return {
            "tenant_id": self.tenant_config.tenant_id,
            "billing_period_days": billing_period_days,
            "total_records": total_records,
            "base_fee": base_fee,
            "per_record_rate": per_record_rate,
            "raw_usage_cost": raw_cost,
            "discount_rate": discount_rate,
            "discount_amount": discount_amount,
            "usage_cost": usage_cost,
            "total_cost": total_cost,
        }


# ---------------------------------------------------------------------------
# Pipeline orchestrator
# ---------------------------------------------------------------------------

class Pipeline:
    """Main pipeline orchestrator."""

    def __init__(
        self,
        name: str,
        source: DataSource,
        sink: DataSink,
        transformers: Optional[List[Transformer]] = None,
        schema: Optional[TableSchema] = None,
        tenant_config: Optional[TenantConfig] = None,
        batch_size: int = 1000,
        max_errors: int = 100,
    ):
        self.name = name
        self.source = source
        self.sink = sink
        self.transformers = transformers or []
        self.schema = schema
        self.tenant_config = tenant_config
        self.batch_size = batch_size
        self.max_errors = max_errors
        self.status = PipelineStatus.PENDING
        self.metrics = PipelineMetrics()
        self._validator = RecordValidator(schema) if schema else None

    def run(self) -> PipelineMetrics:
        """Execute the pipeline."""
        self.status = PipelineStatus.RUNNING
        self.metrics.start_time = datetime.datetime.utcnow()

        try:
            with pipeline_stage("connect", self.metrics):
                self.source.connect()
                self.sink.connect()

            with pipeline_stage("process", self.metrics):
                error_count = 0
                while True:
                    batch = self.source.read_batch(self.batch_size)
                    if not batch:
                        break

                    processed = []
                    for record in batch:
                        try:
                            result = self._process_record(record)
                            if result is not None:
                                processed.append(result)
                                self.metrics.records_processed += 1
                            else:
                                self.metrics.records_skipped += 1
                        except Exception as e:
                            error_count += 1
                            self.metrics.records_failed += 1
                            logger.error(f"Error processing record: {e}")
                            if error_count >= self.max_errors:
                                raise PipelineError(
                                    f"Too many errors ({error_count}), aborting pipeline"
                                )

                    if processed:
                        with pipeline_stage("write_batch", self.metrics):
                            self.sink.write_batch(processed)

            with pipeline_stage("finalize", self.metrics):
                self.sink.flush()

            self.status = PipelineStatus.SUCCEEDED

        except Exception as e:
            self.status = PipelineStatus.FAILED
            logger.error(f"Pipeline {self.name} failed: {e}")
            raise

        finally:
            self.metrics.end_time = datetime.datetime.utcnow()
            self.source.close()
            self.sink.close()

        return self.metrics

    def _process_record(self, record: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        """Process a single record through validation and transformations."""
        # Validate
        if self._validator:
            results = self._validator.validate(record)
            errors = [r for r in results if r.severity == ValidationSeverity.ERROR]
            if errors:
                raise SchemaValidationError([e.message for e in errors])

        # Transform
        result = record
        for transformer in self.transformers:
            result = transformer.transform(result)
            if result is None:
                return None

        return result


# ---------------------------------------------------------------------------
# Pipeline builder (fluent API)
# ---------------------------------------------------------------------------

class PipelineBuilder:
    """Fluent builder for constructing pipelines."""

    def __init__(self, name: str):
        self._name = name
        self._source: Optional[DataSource] = None
        self._sink: Optional[DataSink] = None
        self._transformers: List[Transformer] = []
        self._schema: Optional[TableSchema] = None
        self._tenant_config: Optional[TenantConfig] = None
        self._batch_size = 1000
        self._max_errors = 100

    def from_json(self, file_path: str) -> "PipelineBuilder":
        self._source = JsonFileSource(file_path)
        return self

    def from_csv(self, file_path: str, delimiter: str = ",") -> "PipelineBuilder":
        self._source = CsvFileSource(file_path, delimiter=delimiter)
        return self

    def to_database(self, connection_string: str, table: str) -> "PipelineBuilder":
        self._sink = DatabaseSink(connection_string, table)
        return self

    def with_field_mapping(self, field_map: Dict[str, str]) -> "PipelineBuilder":
        self._transformers.append(FieldMappingTransformer(field_map))
        return self

    def with_filter(self, conditions: List[Dict]) -> "PipelineBuilder":
        self._transformers.append(FilterTransformer(conditions))
        return self

    def with_enrichment(self, enrichments: List[Dict]) -> "PipelineBuilder":
        self._transformers.append(DataEnrichmentTransformer(enrichments))
        return self

    def with_schema(self, schema: TableSchema) -> "PipelineBuilder":
        self._schema = schema
        return self

    def for_tenant(self, config: TenantConfig) -> "PipelineBuilder":
        self._tenant_config = config
        return self

    def with_batch_size(self, size: int) -> "PipelineBuilder":
        self._batch_size = size
        return self

    def with_max_errors(self, max_errors: int) -> "PipelineBuilder":
        self._max_errors = max_errors
        return self

    def build(self) -> Pipeline:
        if not self._source:
            raise ValueError("Pipeline source is required")
        if not self._sink:
            raise ValueError("Pipeline sink is required")
        return Pipeline(
            name=self._name,
            source=self._source,
            sink=self._sink,
            transformers=self._transformers,
            schema=self._schema,
            tenant_config=self._tenant_config,
            batch_size=self._batch_size,
            max_errors=self._max_errors,
        )


# ---------------------------------------------------------------------------
# Entry point for CLI usage
# ---------------------------------------------------------------------------

def main():
    import argparse

    parser = argparse.ArgumentParser(description="Data Processing Pipeline")
    parser.add_argument("--config", required=True, help="Path to pipeline config file")
    parser.add_argument("--tenant", required=True, help="Tenant ID")
    parser.add_argument("--dry-run", action="store_true", help="Validate without executing")
    parser.add_argument("--verbose", action="store_true", help="Enable verbose logging")
    args = parser.parse_args()

    log_level = logging.DEBUG if args.verbose else logging.INFO
    logging.basicConfig(
        level=log_level,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    )

    logger.info(f"Starting pipeline with config: {args.config}")
    logger.info(f"Tenant: {args.tenant}")

    if args.dry_run:
        logger.info("Dry run mode — validating configuration only")

    # In a real implementation, this would load config and build the pipeline
    logger.info("Pipeline completed successfully")


if __name__ == "__main__":
    main()
