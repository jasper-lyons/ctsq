require 'json'
require_relative 'helper'

MAX_SIZE = 64

def main
  result = allocate_buffer(MAX_SIZE)
  result
end

class MyClass
  def run
    main
  end

  def self.build
    new
  end
end

module Shapes
  def area
    0
  end
end
